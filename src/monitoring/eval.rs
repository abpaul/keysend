//! The evaluation of the AI texts of the report on synthetic Ad Accounts.
//!
//! `scripts/ai-eval/run.sh` starts a local Responses API that answers through Codex, runs
//! `writes_the_reports_of_the_scenarios` against it, and scores each report with an
//! independent judge. Each scenario states what a competent consultant concludes from its
//! data; the judge compares the report with that, so no AI answer is its own reference.

pub(crate) mod scenarios;

use std::{collections::BTreeMap, fmt::Write, sync::Arc, sync::Mutex};

use serde_json::{json, Value};

use super::{
    audit::{Audit, AuditRow, Counts, Data},
    business::Business,
    campaign_settings::{AccountDefaults, Resolved, Rules},
    causes::{self, Inputs},
    deep_check::{self, DeepCheck},
    detection,
    metrics::{self, CampaignDay, Metrics, Window},
    tracking, update,
};
use crate::{
    settings::{ServiceSettings, Settings},
    workspace::history::{day_number, Campaign},
};

#[derive(Clone, Copy)]
pub(crate) struct Level {
    pub(crate) spend: f64,
    pub(crate) clicks: f64,
    pub(crate) impressions: f64,
    pub(crate) conversions: f64,
    pub(crate) value: f64,
}

pub(crate) struct Plan {
    pub(crate) id: i64,
    pub(crate) name: &'static str,
    pub(crate) channel: &'static str,
    pub(crate) before: Level,
    pub(crate) after: Level,
    pub(crate) since: i64,
    pub(crate) year_ago: Option<(Level, Level)>,
    pub(crate) lost_to_budget: Option<f64>,
}

#[derive(Clone, Copy)]
pub(crate) struct Row {
    pub(crate) campaign: i64,
    pub(crate) name: &'static str,
    pub(crate) clicks: f64,
    pub(crate) cost: f64,
    pub(crate) conversions: f64,
    pub(crate) value: f64,
    pub(crate) strength: &'static str,
}

/// A Recommendation of an earlier Deep check.
pub(crate) struct Past {
    pub(crate) key: &'static str,
    pub(crate) campaign: i64,
    pub(crate) action: &'static str,
    pub(crate) done: bool,
    pub(crate) opened_ago: i64,
    pub(crate) closed_ago: Option<i64>,
}

#[derive(Default)]
pub(crate) struct Scenario {
    pub(crate) key: &'static str,
    pub(crate) business: &'static str,
    pub(crate) website: &'static str,
    pub(crate) currency: &'static str,
    pub(crate) goal: &'static str,
    pub(crate) target_cost: Option<f64>,
    pub(crate) note: &'static str,
    pub(crate) situation: &'static str,
    pub(crate) essentials: &'static [&'static str],
    pub(crate) mistakes: &'static [&'static str],
    pub(crate) campaigns: Vec<Plan>,
    pub(crate) conversion_actions: u32,
    pub(crate) search_terms: Vec<Row>,
    pub(crate) keywords: Vec<Row>,
    pub(crate) landing_pages: Vec<Row>,
    pub(crate) ads: Vec<Row>,
    pub(crate) locations: Vec<(&'static str, Row)>,
    pub(crate) devices: Vec<(&'static str, Row)>,
    pub(crate) margin: Option<f64>,
    pub(crate) customer_value: Option<f64>,
    pub(crate) close_rate: Option<f64>,
    pub(crate) target_roas: Option<f64>,
    /// The share of conversions in each conversion lag bucket of Google Ads. The history
    /// records only the conversions that have arrived by the end day.
    pub(crate) lag: &'static [(&'static str, f64)],
    pub(crate) history: Vec<Past>,
}

const LAG_DAYS: [(&str, i64); 19] = [
    ("LESS_THAN_ONE_DAY", 1),
    ("ONE_TO_TWO_DAYS", 2),
    ("TWO_TO_THREE_DAYS", 3),
    ("THREE_TO_FOUR_DAYS", 4),
    ("FOUR_TO_FIVE_DAYS", 5),
    ("FIVE_TO_SIX_DAYS", 6),
    ("SIX_TO_SEVEN_DAYS", 7),
    ("SEVEN_TO_EIGHT_DAYS", 8),
    ("EIGHT_TO_NINE_DAYS", 9),
    ("NINE_TO_TEN_DAYS", 10),
    ("TEN_TO_ELEVEN_DAYS", 11),
    ("ELEVEN_TO_TWELVE_DAYS", 12),
    ("TWELVE_TO_THIRTEEN_DAYS", 13),
    ("THIRTEEN_TO_FOURTEEN_DAYS", 14),
    ("FOURTEEN_TO_TWENTY_ONE_DAYS", 21),
    ("TWENTY_ONE_TO_THIRTY_DAYS", 30),
    ("THIRTY_TO_FORTY_FIVE_DAYS", 45),
    ("FORTY_FIVE_TO_SIXTY_DAYS", 60),
    ("SIXTY_TO_NINETY_DAYS", 90),
];

/// The share of a day's conversions that has arrived `age` days after it.
fn arrived(lag: &[(&str, f64)], age: i64) -> f64 {
    let total = lag.iter().map(|(_, share)| share).sum::<f64>();
    if total <= 0.0 {
        return 1.0;
    }
    lag.iter()
        .filter(|(bucket, _)| {
            LAG_DAYS
                .iter()
                .any(|(name, days)| name == bucket && *days <= age)
        })
        .map(|(_, share)| share)
        .sum::<f64>()
        / total
}

const HISTORY_DAYS: i64 = 430;

pub(crate) fn end() -> i64 {
    day_number(chrono::NaiveDate::from_ymd_opt(2026, 10, 15).unwrap())
}

fn variation(day: i64, campaign: i64) -> f64 {
    let weekday = match day.rem_euclid(7) {
        2 | 3 => 0.85,
        _ => 1.06,
    };
    let seed = (day * 7919 + campaign * 104_729).rem_euclid(1000) as f64 / 1000.0;
    weekday * (1.0 + 0.16 * (seed - 0.5))
}

fn history(scenario: &Scenario) -> Vec<CampaignDay> {
    let end = end();
    let mut days = Vec::new();
    for plan in &scenario.campaigns {
        for day in end - HISTORY_DAYS..=end {
            let ago = end - day;
            let level = match plan.year_ago {
                Some((_, after)) if (364..394).contains(&ago) => after,
                Some((before, _)) if (394..424).contains(&ago) => before,
                _ if ago < plan.since => plan.after,
                _ => plan.before,
            };
            let factor = variation(day, plan.id);
            let arrived = arrived(scenario.lag, ago + 1);
            days.push(CampaignDay {
                day,
                campaign_id: plan.id,
                metrics: Metrics {
                    spend: level.spend * factor,
                    clicks: (level.clicks * factor).round(),
                    impressions: (level.impressions * factor).round(),
                    conversions: Some(level.conversions * factor * arrived),
                    conversion_value: Some(level.value * factor * arrived),
                },
            });
        }
    }
    days
}

fn counts(metrics: &Metrics) -> Counts {
    Counts {
        impressions: metrics.impressions,
        clicks: metrics.clicks,
        conversions: metrics.conversions.unwrap_or_default(),
        cost: metrics.spend,
        value: metrics.conversion_value.unwrap_or_default(),
    }
}

fn audit_row(row: &Row, name: &str) -> AuditRow {
    AuditRow {
        name: name.into(),
        id: None,
        counts: Counts {
            impressions: row.clicks * 12.0,
            clicks: row.clicks,
            conversions: row.conversions,
            cost: row.cost,
            value: row.value,
        },
        strength: row.strength.into(),
        ..Default::default()
    }
}

pub(crate) struct Built {
    pub days: Vec<CampaignDay>,
    pub campaigns: Vec<Campaign>,
    pub rules: Resolved,
    pub check: DeepCheck,
    pub first: i64,
}

pub(crate) fn build(scenario: &Scenario) -> Built {
    let end = end();
    let days = history(scenario);
    let total = metrics::sum(&days, end - 29, end, None);
    let state = tracking::state(
        total.conversions.unwrap_or_default(),
        Some(scenario.conversion_actions),
    );
    let rules = Resolved::new(
        AccountDefaults {
            state,
            judged: tracking::judge(state, None),
            target_cost: scenario.target_cost,
            alerts: true,
        },
        BTreeMap::new(),
    );
    let campaigns = scenario
        .campaigns
        .iter()
        .map(|plan| Campaign {
            id: plan.id,
            name: plan.name.into(),
            status: "ENABLED".into(),
            channel_type: plan.channel.into(),
            bidding_strategy: "MAXIMIZE_CONVERSIONS".into(),
            budget_micros: Some((plan.after.spend * 1_000_000.0).round() as i64),
        })
        .collect::<Vec<_>>();
    let mut data = Data {
        currency: scenario.currency.into(),
        reporting: counts(&total),
        audit: Audit {
            conversion_actions: Some(scenario.conversion_actions),
            ..Default::default()
        },
    };
    let mut by_campaign: BTreeMap<i64, Audit> = BTreeMap::new();
    for plan in &scenario.campaigns {
        let row = AuditRow {
            name: plan.name.into(),
            id: Some(plan.id),
            counts: counts(&metrics::sum(&days, end - 29, end, Some(plan.id))),
            lost_to_budget: plan.lost_to_budget,
            ..Default::default()
        };
        data.audit.campaigns.push(row.clone());
        by_campaign.entry(plan.id).or_default().campaigns.push(row);
    }
    for (rows, pick) in [
        (&scenario.search_terms, 0),
        (&scenario.keywords, 1),
        (&scenario.landing_pages, 2),
        (&scenario.ads, 3),
    ] {
        for row in rows {
            let audit_row = audit_row(row, row.name);
            let scoped = by_campaign.entry(row.campaign).or_default();
            let (all, own) = match pick {
                0 => (&mut data.audit.search_terms, &mut scoped.search_terms),
                1 => (&mut data.audit.keywords, &mut scoped.keywords),
                2 => (&mut data.audit.landing_pages, &mut scoped.landing_pages),
                _ => (&mut data.audit.ads, &mut scoped.ads),
            };
            all.push(audit_row.clone());
            own.push(audit_row);
        }
    }
    data.audit.locations = scenario
        .locations
        .iter()
        .map(|(name, row)| audit_row(row, name))
        .collect();
    data.audit.economics = super::economics::Economics {
        margin: scenario.margin,
        customer_value: scenario.customer_value,
        close_rate: scenario.close_rate,
        target_return: scenario.target_roas,
    };
    data.audit.lag = scenario
        .lag
        .iter()
        .map(|(bucket, share)| ((*bucket).to_owned(), *share))
        .collect();
    deep_check::compare(&mut data, &mut by_campaign, &days, end);
    let findings = deep_check::detect(&data, &by_campaign, &rules);
    let first = metrics::first_active_day(&days).unwrap_or(end - HISTORY_DAYS);
    Built {
        check: DeepCheck {
            day: end + 1,
            data,
            by_campaign,
            findings,
            judge_by_conversions: rules.judged(),
            recommendations_generated: false,
            assessment: None,
            research: None,
        },
        days,
        campaigns,
        rules,
        first,
    }
}

fn ratio(numerator: f64, denominator: f64) -> Option<f64> {
    (denominator > 0.0).then(|| numerator / denominator)
}

fn metric_table(scenario: &Scenario, built: &Built) -> String {
    let money = |value: f64| metrics::english_money(value, scenario.currency);
    let rows = |metrics: &Metrics| {
        let conversions = metrics.conversions.unwrap_or_default();
        let value = metrics.conversion_value.unwrap_or_default();
        [
            money(metrics.spend),
            metrics::english_count(metrics.clicks),
            metrics::english_count(metrics.impressions),
            ratio(metrics.clicks * 100.0, metrics.impressions)
                .map_or("-".into(), |rate| format!("{rate:.2}%")),
            ratio(metrics.spend, metrics.clicks).map_or("-".into(), money),
            format!("{conversions:.1}"),
            ratio(metrics.spend, conversions).map_or("-".into(), money),
            money(value),
            ratio(value, metrics.spend).map_or("-".into(), |roas| format!("{roas:.2}")),
        ]
    };
    let mut table = String::from("| Scope | Period | Spend | Clicks | Impressions | CTR | CPC | Conversions | Cost per conversion | Conversion value | ROAS |\n|---|---|---|---|---|---|---|---|---|---|---|\n");
    let end = end();
    let mut scopes = vec![(String::from("Ad Account"), None)];
    scopes.extend(
        scenario
            .campaigns
            .iter()
            .map(|plan| (plan.name.to_owned(), Some(plan.id))),
    );
    for (name, campaign) in scopes {
        let window = metrics::window_metrics(&built.days, end, Window::ThirtyDays, campaign, true);
        for (period, values) in [
            ("30 days before the Deep check", &window.current),
            ("Previous 30 days", &window.previous),
        ] {
            let _ = writeln!(
                table,
                "| {name} | {period} | {} |",
                rows(values).join(" | ")
            );
        }
    }
    table
}

fn breakdown_table(scenario: &Scenario, title: &str, rows: &[(&str, Row)]) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let money = |value: f64| metrics::english_money(value, scenario.currency);
    let mut table = format!("\n### {title}, 30 days before the Deep check\n\n| {title} | Spend | Clicks | Conversions | Cost per conversion |\n|---|---|---|---|---|\n");
    for (name, row) in rows {
        let _ = writeln!(
            table,
            "| {name} | {} | {} | {} | {} |",
            money(row.cost),
            metrics::english_count(row.clicks),
            metrics::english_count(row.conversions),
            ratio(row.cost, row.conversions).map_or("-".into(), money)
        );
    }
    table
}

struct Report {
    text: String,
    ai: Value,
}

async fn report(settings: &Settings, scenario: &Scenario, built: &Built) -> Report {
    let end = end();
    let mut detection = detection::detect(
        &built.days,
        &built.campaigns,
        &[],
        end,
        built.first,
        &built.rules,
        scenario.currency,
    );
    detection.causes = causes::candidates(
        &detection,
        &Inputs {
            days: &built.days,
            changes: &[],
            names: built
                .campaigns
                .iter()
                .map(|campaign| (campaign.id, campaign.name.clone()))
                .collect(),
            shares_now: Vec::new(),
            shares_before: Vec::new(),
            devices: BTreeMap::new(),
        },
    );
    let update = update::generate(settings, None, detection, &built.rules)
        .await
        .expect("an Update");
    let mut business = Business {
        name: scenario.business.into(),
        website: Some(scenario.website.into()),
        goal: scenario.goal.into(),
        note: scenario.note.into(),
        research: None,
    };
    business.research = super::business::research(
        settings,
        None,
        &business,
        &built
            .campaigns
            .iter()
            .map(|campaign| campaign.name.clone())
            .collect::<Vec<_>>(),
        &scenario
            .locations
            .iter()
            .map(|(name, _)| (*name).to_owned())
            .collect::<Vec<_>>(),
        end + 1,
    )
    .await;
    let research = business.research.clone();
    let context = deep_check::Context {
        business,
        campaigns: built.campaigns.clone(),
        blocked: scenario
            .history
            .iter()
            .filter(|past| past.closed_ago.is_none_or(|ago| ago < 90))
            .map(|past| past.key.to_owned())
            .collect(),
        history: scenario
            .history
            .iter()
            .map(|past| super::earlier::Earlier {
                recommendation: crate::workspace::monitoring::Recommendation {
                    finding_key: past.key.into(),
                    action: past.action.into(),
                    reason: String::new(),
                    evidence: String::new(),
                    measure: String::new(),
                    intent: String::new(),
                    subject: String::new(),
                    confidence: Default::default(),
                    campaign_id: Some(past.campaign),
                    state: if past.done {
                        crate::workspace::monitoring::RecommendationState::Done
                    } else {
                        crate::workspace::monitoring::RecommendationState::Open
                    },
                },
                opened_day: end + 1 - past.opened_ago,
                closed_day: past.closed_ago.map(|ago| end + 1 - ago),
                still_detected: !past.done,
            })
            .collect(),
        days: built.days.clone(),
    };
    let review = deep_check::review(settings, None, &built.check, &context, &built.rules).await;
    let (assessment, recommendations) = match review {
        Ok(review) => (Some(review.assessment), review.recommendations),
        Err(error) => {
            tracing::warn!(error = %error, "Review failed");
            (None, Vec::new())
        }
    };
    let names = built
        .campaigns
        .iter()
        .map(|campaign| (campaign.id, campaign.name.as_str()))
        .collect::<BTreeMap<_, _>>();
    let mut text = format!(
        "# {} — Google Ads report\n\nThe Deep check of 16 October 2026 covers the 30 days before it. The Latest Update covers the last 7 days.\n\n## Metrics\n\n{}{}{}",
        scenario.business,
        metric_table(scenario, built),
        breakdown_table(scenario, "Location", &scenario.locations),
        breakdown_table(scenario, "Device", &scenario.devices),
    );
    let state = tracking::state(
        built.check.data.reporting.conversions,
        Some(scenario.conversion_actions),
    );
    if let Some(notice) = super::view::tracking_notice(
        crate::language::english(),
        state,
        None,
        built.rules.judged(),
    ) {
        let _ = write!(text, "\n## {}\n\n{}\n", notice.title, notice.text);
    }
    if let Some(assessment) = &assessment {
        let _ = write!(
            text,
            "\n## Assessment\n\nResult: {}\n\n{}\n",
            assessment.verdict.as_str(),
            assessment.headline
        );
        for (title, points) in [
            ("What worked", &assessment.worked),
            ("Needs attention", &assessment.attention),
        ] {
            if !points.is_empty() {
                let _ = write!(text, "\n### {title}\n");
                for point in points {
                    let _ = write!(text, "\n- {}", point.observation);
                    if let Some(explanation) = &point.explanation {
                        let _ = write!(text, " Possible explanation: {explanation}");
                    }
                }
                text.push('\n');
            }
        }
        if let Some(market) = &assessment.market {
            let _ = write!(text, "\n### Market context\n\n{market}\n");
        }
    }
    let earlier = scenario
        .history
        .iter()
        .filter(|past| !past.done)
        .collect::<Vec<_>>();
    if !earlier.is_empty() {
        text.push_str("\n## Open Recommendations from earlier Deep checks\n");
        for past in earlier {
            let _ = write!(
                text,
                "\n- {} (open since {} days)",
                past.action, past.opened_ago
            );
        }
        text.push('\n');
    }
    if !recommendations.is_empty() {
        text.push_str("\n## Recommendations\n");
    }
    for (index, recommendation) in recommendations.iter().enumerate() {
        let _ = write!(
            text,
            "\n{}. {}\n   About: {}\n   Why: {}\n   How to check: {}\n   Confidence: {}\n   Evidence: {}\n",
            index + 1,
            recommendation.action,
            if recommendation.subject.is_empty() { "the Ad Account" } else { &recommendation.subject },
            recommendation.reason,
            recommendation.measure,
            recommendation.confidence.as_str(),
            recommendation.evidence,
        );
    }
    if let Some(assessment) = assessment
        .as_ref()
        .filter(|assessment| !assessment.next.is_empty())
    {
        text.push_str("\n## Next 30 days\n");
        for point in &assessment.next {
            let _ = write!(text, "\n- {point}");
        }
        text.push('\n');
    }
    let _ = write!(
        text,
        "\n## Latest Update (last 7 days)\n\n{}\n",
        update.summary
    );
    if let Some(cause) = &update.cause {
        let _ = writeln!(text, "\n{:?} cause: {}", cause.level, cause.evidence);
    }
    for line in &update.lines {
        let _ = write!(
            text,
            "\n- {}: {}",
            names.get(&line.campaign_id).unwrap_or(&"?"),
            line.text
        );
        if let Some(cause) = &line.cause {
            let _ = write!(text, " ({:?} cause: {})", cause.level, cause.evidence);
        }
    }
    Report {
        ai: json!({
            "update_written_by_ai": !update.written_without_ai,
            "research": research,
            "assessment_written_by_ai": assessment.as_ref().is_some_and(|assessment| !assessment.written_without_ai),
            "recommendations": recommendations.len(),
            "findings": built.check.findings.iter().map(|finding| &finding.rule.key).collect::<Vec<_>>(),
            "complete_recommendations": recommendations.iter().filter(|recommendation| !recommendation.measure.is_empty() && !recommendation.evidence.is_empty() && (!recommendation.subject.is_empty() || recommendation.finding_key.starts_with("tracking"))).count(),
            "explanations": assessment.as_ref().map_or(0, |assessment| assessment.worked.iter().chain(&assessment.attention).filter(|point| point.explanation.is_some()).count()),
            "points": assessment.as_ref().map_or(0, |assessment| assessment.worked.len() + assessment.attention.len()),
        }),
        text,
    }
}

pub(crate) fn settings(url: &str) -> Settings {
    Settings {
        postdeploy: Default::default(),
        postdeploy_key: None,
        billing: Default::default(),
        stripe_key: None,
        stripe_live: false,
        stripe_webhook: None,
        services: ServiceSettings {
            ai_url: url.into(),
            ..Default::default()
        },
        monitoring: Default::default(),
        agent: Default::default(),
        optimize: Default::default(),
        openai_key: Some("evaluation".into()),
        email_key: None,
        email_recipient: None,
        google: Default::default(),
        client_id: None,
        client_secret: None,
        master_key: None,
        admin_emails: Vec::new(),
        origin: "http://localhost".into(),
        redirect_uri: String::new(),
        secure: false,
        http: reqwest::Client::new(),
    }
}

#[derive(Clone)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn run(scenario: &Scenario, trial: usize, url: &str, directory: &std::path::Path) {
    let warnings = Captured(Arc::new(Mutex::new(Vec::new())));
    let writer = warnings.clone();
    let _logs = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish(),
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let built = build(scenario);
    let started = std::time::Instant::now();
    let report = runtime.block_on(report(&settings(url), scenario, &built));
    let seconds = started.elapsed().as_secs_f64();
    let warnings = String::from_utf8_lossy(&warnings.0.lock().unwrap()).into_owned();
    let result = json!({
        "scenario": scenario.key,
        "trial": trial,
        "business": scenario.business,
        "website": scenario.website,
        "goal": scenario.goal,
        "target_cost": scenario.target_cost,
        "note": scenario.note,
        "margin": scenario.margin,
        "customer_value": scenario.customer_value,
        "close_rate": scenario.close_rate,
        "target_roas": scenario.target_roas,
        "history": scenario.history.iter().map(|past| json!({"campaign": past.campaign, "action": past.action, "state": if past.done { "done" } else { "open" }, "opened_days_ago": past.opened_ago, "closed_days_ago": past.closed_ago})).collect::<Vec<_>>(),
        "situation": scenario.situation,
        "essentials": scenario.essentials,
        "mistakes": scenario.mistakes,
        "report": report.text,
        "ai": report.ai,
        "warnings": warnings.lines().collect::<Vec<_>>(),
        "seconds": seconds,
    });
    std::fs::write(
        directory.join(format!("{}.{trial}.json", scenario.key)),
        serde_json::to_string_pretty(&result).unwrap(),
    )
    .unwrap();
    std::fs::write(
        directory.join(format!("{}.{trial}.md", scenario.key)),
        &report.text,
    )
    .unwrap();
}

#[test]
#[ignore = "calls the AI through KEYSEND_EVAL_AI_URL; run scripts/ai-eval/run.sh"]
fn writes_the_reports_of_the_scenarios() {
    let url = std::env::var("KEYSEND_EVAL_AI_URL").expect("set KEYSEND_EVAL_AI_URL");
    let directory =
        std::path::PathBuf::from(std::env::var("KEYSEND_EVAL_DIR").expect("set KEYSEND_EVAL_DIR"));
    let trials = std::env::var("KEYSEND_EVAL_TRIALS")
        .ok()
        .and_then(|trials| trials.parse().ok())
        .unwrap_or(1);
    let only = std::env::var("KEYSEND_EVAL_ONLY").unwrap_or_default();
    std::fs::create_dir_all(&directory).unwrap();
    let scenarios = scenarios::all()
        .into_iter()
        .filter(|scenario| only.is_empty() || only.split(',').any(|key| key == scenario.key))
        .collect::<Vec<_>>();
    std::thread::scope(|scope| {
        for scenario in &scenarios {
            for trial in 1..=trials {
                let (url, directory) = (url.as_str(), directory.as_path());
                scope.spawn(move || run(scenario, trial, url, directory));
            }
        }
    });
}

#[test]
fn every_scenario_builds_an_account_history_and_a_deep_check() {
    for scenario in scenarios::all() {
        let built = build(&scenario);
        assert!(
            built.check.data.reporting.clicks > 0.0,
            "{} has no clicks",
            scenario.key
        );
        assert!(!scenario.essentials.is_empty(), "{}", scenario.key);
    }
}

#[test]
fn the_scenarios_produce_the_findings_that_their_data_supports() {
    let found = |key: &str| {
        let scenario = scenarios::all()
            .into_iter()
            .find(|scenario| scenario.key == key)
            .unwrap();
        let mut keys = build(&scenario)
            .check
            .findings
            .into_iter()
            .map(|finding| finding.rule.key.split(':').next().unwrap().to_owned())
            .collect::<Vec<_>>();
        keys.sort();
        keys.dedup();
        keys
    };
    assert_eq!(
        found("high_ctr_poor_conversion"),
        [
            "ads-poor-strength",
            "landing-page-low-rate",
            "traffic-without-results"
        ]
    );
    assert_eq!(
        found("rising_cpc_stable_conversions"),
        ["click-cost-rising"]
    );
    assert_eq!(
        found("irrelevant_search_terms"),
        ["search-terms-no-conversions", "traffic-without-results"]
    );
    assert_eq!(found("incomplete_tracking"), ["tracking-stopped"]);
    assert_eq!(found("multi_region_local"), ["location-cost-high"]);
    assert_eq!(
        found("misleading_roas"),
        ["campaign-return-low", "search-terms-no-conversions"]
    );
    assert_eq!(found("healthy_account"), Vec::<String>::new());
    assert_eq!(found("double_counted_conversions"), ["tracking-spike"]);
    assert_eq!(found("conversion_lag_dip"), Vec::<String>::new());
    assert_eq!(
        found("margin_below_break_even"),
        ["campaign-below-break-even"]
    );
    assert_eq!(
        found("lead_value_break_even"),
        ["campaign-above-break-even"]
    );
}

#[test]
#[ignore = "researches KEYSEND_EVAL_RESEARCH_NAME at KEYSEND_EVAL_RESEARCH_SITE through KEYSEND_EVAL_AI_URL"]
fn researches_a_real_business_and_verifies_its_facts_on_their_pages() {
    let variable = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("set {name}"));
    let business = Business {
        name: variable("KEYSEND_EVAL_RESEARCH_NAME"),
        website: Some(variable("KEYSEND_EVAL_RESEARCH_SITE")),
        ..Default::default()
    };
    let research = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(super::business::research(
            &settings(&variable("KEYSEND_EVAL_AI_URL")),
            None,
            &business,
            &[],
            &[],
            end() + 1,
        ))
        .expect("an answer");
    println!("{}", serde_json::to_string_pretty(&research).unwrap());
}
