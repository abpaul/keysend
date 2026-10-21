//! Paired evaluations of production investigation and answer verification on synthetic data.
mod held_out;
mod held_out_2;
mod scenarios;

use super::{
    data::{AccountData, DeviceData, Scope, ScopedAccount},
    evidence::{Evidence, Strength},
    investigation::{self, Journal, Limits, Reply, Request},
    operation::{CampaignOrder, Operation, Period, SearchTermFocus, Segment},
    run, Handle,
};
use crate::{
    monitoring::{detection, eval as history, metrics},
    workspace::{
        investigations::{InvestigationStep, Outcome},
        profile::Profile,
    },
};
use async_trait::async_trait;
use scenarios::Scenario;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    time::Instant,
};

fn build(scenario: &Scenario) -> (Scope, BTreeMap<Handle, AccountData>) {
    let mut accounts = Vec::new();
    let mut data = BTreeMap::new();
    for (index, source) in scenario.accounts.iter().enumerate() {
        let handle = Handle::new(index);
        let customer_id = format!("synthetic-{index}");
        accounts.push(ScopedAccount {
            handle,
            customer_id: customer_id.clone(),
            name: source.business.into(),
        });
        data.insert(handle, snapshot(source, customer_id));
    }
    (Scope::synthetic(accounts), data)
}

/// The snapshot of one synthetic Ad Account, as `data::load` reads it from the store.
pub(crate) fn snapshot(source: &history::Scenario, customer_id: String) -> AccountData {
    let built = history::build(source);
    let mut detected = detection::detect(
        &built.days,
        &built.campaigns,
        &[],
        history::end(),
        built.first,
        &built.rules,
        source.currency,
    );
    detected.causes = crate::monitoring::causes::candidates(
        &detected,
        &crate::monitoring::causes::Inputs {
            days: &built.days,
            changes: &[],
            names: built
                .campaigns
                .iter()
                .map(|c| (c.id, c.name.clone()))
                .collect(),
            shares_now: Vec::new(),
            shares_before: Vec::new(),
            devices: BTreeMap::new(),
        },
    );
    AccountData {
        customer_id,
        name: source.business.into(),
        currency: source.currency.into(),
        end_day: history::end(),
        first_day: Some(built.first),
        days: built.days,
        campaigns: built.campaigns,
        changes: Vec::new(),
        rules: built.rules,
        profile: Profile {
            goal: source.goal.into(),
            target_cost: source.target_cost,
            note: source.note.into(),
            economics: built.check.data.audit.economics.clone(),
        },
        findings: built.check.findings.clone(),
        finding_history: built
            .check
            .findings
            .iter()
            .cloned()
            .map(|f| (f, true))
            .collect(),
        earlier: Vec::new(),
        recommendations: Vec::new(),
        action_weeks: Vec::new(),
        devices: source
            .devices
            .iter()
            .enumerate()
            .map(|(i, (_, row))| DeviceData {
                device: i as i64 + 1,
                current: metrics::Metrics {
                    spend: row.cost,
                    clicks: row.clicks,
                    conversions: Some(row.conversions),
                    conversion_value: Some(row.value),
                    ..Default::default()
                },
                previous: None,
            })
            .collect(),
        locations: Vec::new(),
        hours: Vec::new(),
        demographics: Vec::new(),
        conversion_delay: None,
        detection: detected,
        deep_check: Some(built.check),
    }
}

fn operations(scope: &Scope, period: Period) -> Vec<Operation> {
    let mut all = vec![Operation::RankAccounts { period }];
    for account in scope.accounts() {
        let account = account.handle;
        all.extend([
            Operation::AccountOverview { account, period },
            Operation::InvestigateConversions { account },
            Operation::ReviewEconomics { account },
            Operation::ReviewHistory { account },
            Operation::BusinessContext { account },
        ]);
        all.push(Operation::ComparePeriods { account, period });
        for order in [
            CampaignOrder::Spend,
            CampaignOrder::SpendChange,
            CampaignOrder::ConversionChange,
            CampaignOrder::CostPerConversionChange,
        ] {
            all.push(Operation::CompareCampaigns {
                account,
                period,
                order,
                rows: 20.try_into().unwrap(),
            });
        }
        for focus in [
            SearchTermFocus::TopSpend,
            SearchTermFocus::SpendWithoutConversions,
        ] {
            all.push(Operation::InvestigateSearchTerms {
                account,
                campaign: None,
                focus,
                rows: 20.try_into().unwrap(),
            });
        }
        for segment in [
            Segment::Device,
            Segment::Location,
            Segment::Hour,
            Segment::Demographic,
        ] {
            all.push(Operation::InvestigateSegments { account, segment });
        }
    }
    all
}

fn precompute(
    scope: &Scope,
    data: &BTreeMap<Handle, AccountData>,
    period: Period,
) -> Vec<InvestigationStep> {
    let mut ops = operations(scope, period);
    for account in scope.accounts() {
        for campaign in &data[&account.handle].campaigns {
            for focus in [
                SearchTermFocus::TopSpend,
                SearchTermFocus::SpendWithoutConversions,
            ] {
                ops.push(Operation::InvestigateSearchTerms {
                    account: account.handle,
                    campaign: Some(campaign.id),
                    focus,
                    rows: 20.try_into().unwrap(),
                });
            }
        }
    }
    ops.into_iter()
        .enumerate()
        .map(|(index, operation)| {
            let start = Instant::now();
            let (outcome, evidence) = match run::run(&operation, index + 1, scope, data) {
                Ok(e) => (Outcome::Answered, e),
                Err(_) => (Outcome::Refused, Vec::new()),
            };
            InvestigationStep {
                ordinal: index as i64 + 1,
                round: 0,
                operation,
                outcome,
                evidence,
                elapsed_ms: start.elapsed().as_millis() as i64,
            }
        })
        .collect()
}

#[derive(Default)]
struct Memory(Vec<InvestigationStep>);
#[async_trait]
impl Journal for Memory {
    async fn step(&mut self, step: &InvestigationStep) {
        self.0.push(step.clone());
    }
    async fn answering(&mut self) {}
    async fn cancelled(&mut self) -> bool {
        false
    }
}

fn operation_name(operation: &Operation) -> String {
    serde_json::to_value(operation).unwrap()["operation"]
        .as_str()
        .unwrap()
        .into()
}

fn unsupported(text: &str, evidence: &[&Evidence]) -> Vec<String> {
    let facts = json!(evidence
        .iter()
        .map(|e| json!({"subject": e.subject,"facts":e.facts}))
        .collect::<Vec<_>>());
    let known = crate::ai::numbers(&facts.to_string());
    crate::ai::numbers(text)
        .difference(&known)
        .cloned()
        .collect()
}

fn metrics(
    report: &Value,
    evidence: &[Evidence],
    steps: &[InvestigationStep],
    scenario: &Scenario,
    unchecked: bool,
) -> Value {
    let used = steps
        .iter()
        .map(|s| operation_name(&s.operation))
        .collect::<BTreeSet<_>>();
    let expected = scenario
        .expected
        .iter()
        .filter(|op| used.contains(**op))
        .count();
    let mut violations = Vec::new();

    let cited = |claim: &Value| {
        claim["evidence"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|id| {
                evidence
                    .iter()
                    .find(|e| e.id.0 == id.as_str().unwrap_or_default())
            })
            .collect::<Vec<_>>()
    };
    for key in ["observations", "hypotheses", "recommendations"] {
        for claim in report[key].as_array().into_iter().flatten() {
            let e = cited(claim);
            for field in ["text", "action", "target", "rationale", "risk", "measure"] {
                if let Some(text) = claim[field].as_str() {
                    if field == "target" && e.iter().any(|item| text == String::from(item.account))
                    {
                        continue;
                    }
                    violations.extend(unsupported(text, &e));
                }
            }
        }
    }
    // Summary and stated limitations cite the union of the claims' evidence in production.
    let union: Vec<&Evidence> = if unchecked {
        let ids = ["observations", "hypotheses", "recommendations"]
            .iter()
            .flat_map(|key| report[key].as_array().into_iter().flatten())
            .flat_map(|claim| claim["evidence"].as_array().into_iter().flatten())
            .filter_map(Value::as_str)
            .collect::<BTreeSet<_>>();
        evidence
            .iter()
            .filter(|e| ids.contains(e.id.0.as_str()))
            .collect()
    } else {
        report["evidence"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| evidence.iter().find(|e| v["id"] == e.id.0))
            .collect()
    };
    if let Some(summary) = report["summary"].as_str() {
        violations.extend(unsupported(summary, &union));
    }
    for limitation in report["limitations"].as_array().into_iter().flatten() {
        if let Some(text) = limitation.as_str().or_else(|| limitation["text"].as_str()) {
            violations.extend(unsupported(text, &union));
        }
    }
    let weak_actions = report["recommendations"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| r["kind"] == "act" && cited(r).iter().any(|e| e.strength == Strength::Weak))
        .count();
    let pause_all = report["recommendations"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|r| {
            let text = r["action"].as_str().unwrap_or_default().to_lowercase();
            (text.contains("pause") || text.contains("stop") || text.contains("disable"))
                && text.contains("all")
                && text.contains("campaign")
        });
    json!({"expected_recall": if scenario.expected.is_empty() {1.0} else {expected as f64/scenario.expected.len() as f64},
        "unneeded": scenario.unneeded.iter().filter(|op| used.contains(**op)).collect::<Vec<_>>(),
        "repeated": steps.iter().filter(|s| s.outcome == Outcome::Repeated).count(),
        "rounds_with_operations": steps.iter().map(|s| s.round).collect::<BTreeSet<_>>().len(),
        "unsupported_numbers": violations,"weak_actions":weak_actions,"pause_all_campaigns":pause_all})
}

#[derive(Clone, Default)]
struct Capture(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
impl std::io::Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn execute(scenario: &Scenario, trial: usize, arm: &str, url: &str, directory: &Path) {
    let logs = Capture::default();
    let writer = logs.clone();
    let _logs = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish(),
    );
    let (scope, data) = build(scenario);
    let mut journal = Memory::default();
    let mut limits = Limits::default();
    let done = if arm == "dump" {
        limits.operations = 0;
        limits.rounds = 0;
        precompute(&scope, &data, scenario.period)
    } else {
        Vec::new()
    };
    journal.0 = done.clone();
    let mut settings = history::settings(url);
    settings.services.ai_prices.insert(
        settings.services.ai_model.clone(),
        crate::settings::AiPrice {
            input_usd_per_million: 2.0,
            output_usd_per_million: 10.0,
        },
    );
    if let Ok(model) = std::env::var("KEYSEND_EVAL_MODEL") {
        settings.services.ai_model = model;
    }
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "x-keysend-eval-run",
        format!("{}.{trial}.{arm}", scenario.key).parse().unwrap(),
    );
    settings.http = reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .unwrap();
    let started = Instant::now();
    let finished = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(investigation::investigate(
            &settings,
            None,
            &limits,
            Request {
                question: scenario.question,
                period: scenario.period,
                today: "2026-10-16".into(),
                scope: &scope,
                data: &data,
                earlier: None,
                done,
            },
            &mut journal,
        ));
    let seconds = started.elapsed().as_secs_f64();
    let evidence = journal
        .0
        .iter()
        .flat_map(|s| s.evidence.clone())
        .collect::<Vec<_>>();
    let report = match &finished.reply {
        Some(Reply::Answer(report)) => json!(report),
        _ => json!(finished.reply),
    };
    let write = |arm: &str, report: Value| {
        let drafted = finished.draft.as_ref().map_or(0, |d| {
            1 + [
                "observations",
                "hypotheses",
                "recommendations",
                "limitations",
            ]
            .iter()
            .map(|key| d[key].as_array().map_or(0, Vec::len))
            .sum::<usize>()
        });
        let result = json!({"scenario":scenario.key,"held_out":scenario.held_out,"trial":trial,"arm":arm,"model":settings.services.ai_model,
            "question":scenario.question,"period":scenario.period,"situation":scenario.situation,"essentials":scenario.essentials,"mistakes":scenario.mistakes,
            "expected":scenario.expected,"unneeded":scenario.unneeded,"steps":journal.0,"evidence":evidence,"report":report,"draft":finished.draft,
            "state":finished.state,"failure":finished.failure,"usage":finished.usage,"seconds":seconds,
            "check_log": String::from_utf8_lossy(&logs.0.lock().unwrap()).lines().collect::<Vec<_>>(),
            "drafted_texts":drafted,"refused":if arm=="unchecked" {0} else {report["refused"].as_u64().unwrap_or_default()},
            "clarification":matches!(finished.reply,Some(Reply::Clarification{..})),
            "metrics":metrics(&report,&evidence,&journal.0,scenario,arm=="unchecked")});
        std::fs::write(
            directory.join(format!("{}.{trial}.{arm}.json", scenario.key)),
            serde_json::to_string_pretty(&result).unwrap(),
        )
        .unwrap();
    };
    write(arm, report);
    if arm == "agent" {
        write(
            "unchecked",
            finished
                .draft
                .clone()
                .unwrap_or_else(|| json!(finished.reply)),
        );
    }
}

#[test]
#[ignore = "real model calls; scripts/ai-eval/agent-run.sh"]
fn the_agent_answers_the_scenarios() {
    let url = std::env::var("KEYSEND_EVAL_AI_URL").expect("KEYSEND_EVAL_AI_URL");
    let directory =
        std::path::PathBuf::from(std::env::var("KEYSEND_EVAL_DIR").expect("KEYSEND_EVAL_DIR"));
    std::fs::create_dir_all(&directory).unwrap();
    let trials = std::env::var("KEYSEND_EVAL_TRIALS")
        .unwrap_or("2".into())
        .parse::<usize>()
        .unwrap();
    let only = std::env::var("KEYSEND_EVAL_ONLY").unwrap_or_default();
    let cases = scenarios::all()
        .into_iter()
        .filter(|s| only.is_empty() || only.split(',').any(|key| key == s.key))
        .collect::<Vec<_>>();
    // Limit concurrent writers so local CLI startup does not consume the whole time budget.
    for trial in 1..=trials {
        for batch in cases.chunks(3) {
            std::thread::scope(|threads| {
                for scenario in batch {
                    for arm in ["agent", "dump"] {
                        let (url, directory) = (&url, &directory);
                        threads.spawn(move || execute(scenario, trial, arm, url, directory));
                    }
                }
            });
        }
    }
}

fn evidence_case(key: &str, operation: Value) -> Vec<Evidence> {
    let scenario = scenarios::all().into_iter().find(|s| s.key == key).unwrap();
    let (scope, data) = build(&scenario);
    run::run(
        &serde_json::from_value(operation).unwrap(),
        1,
        &scope,
        &data,
    )
    .unwrap()
}

#[test]
fn the_user_sees_stopped_tracking_in_the_ad_accounts_evidence() {
    let rows = evidence_case(
        "C_tracking",
        json!({"operation":"investigate_conversions","account":"A1"}),
    );
    assert_eq!(rows[0].facts["Conversion tracking"], "stopped");
    assert!(rows[0]
        .caveats
        .contains(&super::evidence::Caveat::TrackingNotWorking));
    let rows = evidence_case(
        "C_tracking",
        json!({"operation":"account_overview","account":"A1","period":"last_30_days"}),
    );
    assert_eq!(rows[0].strength, Strength::Weak);
    assert!(rows[0]
        .caveats
        .contains(&super::evidence::Caveat::TrackingNotWorking));
}

#[test]
fn the_user_sees_small_conversion_counts_without_a_broken_tracking_warning() {
    let rows = evidence_case(
        "G_low_volume",
        json!({"operation":"account_overview","account":"A1","period":"last_30_days"}),
    );
    assert_eq!(rows[0].facts["Conversions"], "4.8");
    assert_eq!(rows[0].facts["Baseline"]["Conversions"], "8.1");
    assert!(rows[0]
        .caveats
        .contains(&super::evidence::Caveat::FewConversions { conversions: 5 }));
    assert!(!rows[0]
        .caveats
        .contains(&super::evidence::Caveat::TrackingNotWorking));
    assert_eq!(rows[0].strength, Strength::Weak);
}

#[test]
fn the_user_sees_named_devices_with_efficiency_and_spend_and_conversion_shares() {
    let rows = evidence_case(
        "N_devices",
        json!({"operation":"investigate_segments","account":"A1","segment":"device"}),
    );
    assert_eq!(rows[0].subject.name(), Some("Mobile"));
    assert_eq!(rows[0].facts["Cost per conversion"], "$200");
    assert_eq!(rows[0].facts["Share of spend"], "90%");
    assert_eq!(rows[0].facts["Share of conversions"], "47%");
    assert_eq!(rows[1].facts["Cost per conversion"], "$20.00");
    assert!(rows[0]
        .caveats
        .contains(&super::evidence::Caveat::NoBaseline));
}

#[test]
fn the_user_sees_the_total_spend_of_listed_zero_conversion_search_terms() {
    let rows = evidence_case(
        "F_terms",
        json!({"operation":"investigate_search_terms","account":"A1","campaign":null,"focus":"spend_without_conversions","rows":20}),
    );
    let aggregate = rows
        .iter()
        .find(|r| r.facts["Selection"] == "Listed zero-conversion search terms");
    assert_eq!(
        aggregate.map(|r| r.facts["Spend"].clone()),
        Some(json!("$2,110"))
    );
    let aggregate = aggregate.unwrap();
    assert_eq!(aggregate.facts["Clicks"], "1,250");
    assert_eq!(aggregate.facts["Share of spend"], "46%");
    assert!(aggregate
        .caveats
        .contains(&super::evidence::Caveat::SearchTermsIncomplete {
            shown: 5,
            of_spend_share: Some(46)
        }));
    for row in rows
        .iter()
        .filter(|r| matches!(r.subject, super::evidence::Subject::SearchTerm { .. }))
    {
        if row.facts["Clicks"] != "90" {
            assert_ne!(row.strength, Strength::Weak);
        }
        assert!(!row
            .caveats
            .iter()
            .any(|c| matches!(c, super::evidence::Caveat::FewConversions { .. })));
        assert!(row.caveats.iter().any(|c| matches!(
            c,
            super::evidence::Caveat::SearchTermsIncomplete {
                of_spend_share: Some(_),
                ..
            }
        )));
    }
}

#[test]
fn the_user_sees_only_the_listed_search_terms_total_within_the_row_limit() {
    let scenario = scenarios::all()
        .into_iter()
        .find(|s| s.key == "F_terms")
        .unwrap();
    let (scope, mut data) = build(&scenario);
    let deep = data
        .get_mut(&Handle::new(0))
        .unwrap()
        .deep_check
        .as_mut()
        .unwrap();
    deep.data.reporting.cost = 4000.0;
    deep.by_campaign.get_mut(&31).unwrap().campaigns[0]
        .counts
        .cost = 2000.0;
    for (campaign, share) in [(None, "18%"), (Some(31), "35%")] {
        for limit in [1, 2] {
            let op = serde_json::from_value(json!({"operation":"investigate_search_terms","account":"A1","campaign":campaign,"focus":"spend_without_conversions","rows":limit})).unwrap();
            let rows = run::run(&op, 1, &scope, &data).unwrap();
            assert_eq!(rows.len(), limit as usize);
            assert_eq!(rows[0].facts["Spend"], "$700");
            assert_eq!(rows[0].facts["Clicks"], "420");
            assert_eq!(rows[0].facts["Share of spend"], share);
            assert!(rows[0]
                .caveats
                .contains(&super::evidence::Caveat::RowsLeftOut { rows: 4 }));
        }
    }
}

#[test]
fn the_user_gets_both_comparison_baselines_in_one_operation() {
    let rows = evidence_case(
        "M_seasonal",
        json!({"operation":"compare_periods","account":"A1","period":"last_30_days"}),
    );
    let accounts = rows
        .iter()
        .filter(|r| matches!(r.subject, super::evidence::Subject::AdAccount))
        .collect::<Vec<_>>();
    assert_eq!(accounts.len(), 2);
    assert_eq!(accounts[0].facts["Comparison"], "previous period");
    assert_eq!(accounts[1].facts["Comparison"], "same days last year");
    assert!(accounts.iter().all(|r| r.baseline.is_some()));
}

#[test]
fn the_user_can_cite_profile_targets_and_economics_without_a_measured_baseline() {
    let rows = evidence_case(
        "E_budget",
        json!({"operation":"compare_periods","account":"A1","period":"last_30_days"}),
    );
    assert_eq!(rows[0].facts["Break-even cost per conversion"], "EUR 100");
    let rows = evidence_case(
        "A_cpc",
        json!({"operation":"compare_campaigns","account":"A1","period":"last_30_days","order":"spend","rows":20}),
    );
    assert_eq!(
        rows.iter()
            .find(|r| matches!(r.subject, super::evidence::Subject::AdAccount))
            .map(|r| r.facts["Target cost per conversion"].clone()),
        Some(json!("$60.00"))
    );
    let rows = evidence_case(
        "E_budget",
        json!({"operation":"review_economics","account":"A1"}),
    );
    assert_eq!(rows[0].strength, Strength::Strong);
    assert!(!rows[0]
        .caveats
        .contains(&super::evidence::Caveat::NoBaseline));
}

#[test]
fn the_user_can_read_the_ad_account_profile_note_as_a_fact() {
    let rows = evidence_case(
        "L_website",
        json!({"operation":"account_overview","account":"A1","period":"last_30_days"}),
    );
    assert_eq!(rows[0].facts["Note"], "Emergency plumbing and boiler repair in Leeds. New ads and an offers page went live at the start of the month.");
    assert_eq!(rows[0].facts["Goal"], "calls");
}

#[test]
fn the_user_can_read_the_dates_of_every_evidence_period() {
    let rows = evidence_case(
        "A_cpc",
        json!({"operation":"account_overview","account":"A1","period":"last_30_days"}),
    );
    assert_eq!(rows[0].facts["Period"], "16 September to 15 October 2026");
    assert_eq!(
        rows[0].facts["Baseline period"],
        "17 August to 15 September 2026"
    );
}

#[test]
fn the_scenarios_produce_the_evidence_the_user_needs() {
    for scenario in scenarios::all() {
        let (scope, data) = build(&scenario);
        let steps = precompute(&scope, &data, scenario.period);
        assert!(
            steps.iter().all(|s| s.outcome == Outcome::Answered),
            "{}",
            scenario.key
        );
        assert!(!scenario.essentials.is_empty());
        let evidence = steps.iter().flat_map(|s| &s.evidence).collect::<Vec<_>>();
        for row in &evidence {
            assert_eq!(row.facts["Period"].is_string(), row.days.is_some());
            assert_eq!(
                row.facts["Baseline period"].is_string(),
                row.baseline.is_some()
            );
        }
        let facts = serde_json::to_string(&evidence).unwrap();
        for account in scope.accounts() {
            let d = &data[&account.handle];
            let days = if scenario.period == Period::Last7Days {
                7
            } else {
                30
            };
            let expected = metrics::sum(&d.days, d.end_day - days + 1, d.end_day, None);
            let rows = run::run(
                &Operation::AccountOverview {
                    account: account.handle,
                    period: scenario.period,
                },
                1,
                &scope,
                &data,
            )
            .unwrap();
            assert_eq!(
                rows[0].facts["Spend"],
                metrics::english_money(expected.spend, &d.currency),
                "{}",
                scenario.key
            );
        }
        match scenario.key {
            "A_cpc" => {
                let d = &data[&Handle::new(0)];
                let now = metrics::sum(&d.days, d.end_day - 29, d.end_day, None);
                let before = metrics::sum(&d.days, d.end_day - 59, d.end_day - 30, None);
                assert!((now.spend / now.clicks - 4.5).abs() < 0.02);
                assert!((before.spend / before.clicks - 3.0).abs() < 0.02);
                assert!((now.conversions.unwrap() / now.clicks - 0.06).abs() < 0.001);
                assert!(evidence.iter().any(|e| e.facts["CPC"]
                    .as_str()
                    .is_some_and(|s| s.starts_with('$'))
                    || e.facts["Cost per click"]
                        .as_str()
                        .is_some_and(|s| s.starts_with('$'))));
            }
            "B_conversion_rate" => {
                assert!(facts.contains("Search - Vet Clinic"));
                assert!(facts.contains("Search - Emergency Vet"));
                assert!(facts.contains("Search - Brand"));
                let d = &data[&Handle::new(0)];
                for (id, expected) in [(111, 1.0 / 3.0), (112, 1.0 / 3.0), (113, 1.0)] {
                    let now = metrics::sum(&d.days, d.end_day - 29, d.end_day, Some(id));
                    let before = metrics::sum(&d.days, d.end_day - 59, d.end_day - 30, Some(id));
                    assert!((now.clicks / before.clicks - 1.0).abs() < 0.02);
                    assert!(
                        (now.conversions.unwrap() / before.conversions.unwrap() - expected).abs()
                            < 0.02
                    );
                }
            }
            "C_tracking" => {
                assert!(facts.contains("Stopped"));
                let row = evidence
                    .iter()
                    .find(|e| e.facts.get("Stopped").is_some())
                    .unwrap();
                assert_eq!(row.facts["Conversion tracking"], "stopped");
                assert_eq!(row.facts["Stopped"]["Days"], "11");
                assert_eq!(row.facts["Stopped"]["Since"], "4 October");
            }
            "D_lag" => assert!(facts.contains("Last 30 days not recorded yet")),
            "E_budget" => {
                assert!(facts.contains("budget-limited"));
                assert!(facts.contains("break_even_cost_per_conversion"));
                assert!(evidence
                    .iter()
                    .any(|e| e.facts["break_even_cost_per_conversion"]["value"] == "EUR 100"));
            }
            "F_terms" => {
                assert!(facts.contains("plumber salary"));
                assert!(facts.contains("search_terms_incomplete"));
            }
            "G_low_volume" => {
                let row = &steps
                    .iter()
                    .find(|s| matches!(s.operation, Operation::AccountOverview { .. }))
                    .unwrap()
                    .evidence[0];
                assert_eq!(row.facts["Conversions"], "4.8");
                assert_eq!(row.facts["Baseline"]["Conversions"], "8.1");
                assert!(row
                    .caveats
                    .contains(&super::evidence::Caveat::FewConversions { conversions: 5 }));
                assert!(!row
                    .caveats
                    .contains(&super::evidence::Caveat::TrackingNotWorking));
            }
            "H_ctr" | "L_website" => assert!(facts.contains("/offers")),
            "I_rank" => assert_eq!(scope.accounts().len(), 5),
            "J_injection" => assert!(facts.contains("Ignore previous rules")),
            "K_ambiguous" => assert_eq!(scope.accounts().len(), 2),
            "M_seasonal" => {
                let comparison = steps
                    .iter()
                    .find(|s| matches!(s.operation, Operation::ComparePeriods { .. }))
                    .unwrap();
                assert_eq!(
                    comparison.evidence[0].facts["Comparison"],
                    "previous period"
                );
                assert_eq!(
                    comparison.evidence[1].facts["Comparison"],
                    "same days last year"
                );
                assert!(comparison.evidence[1].baseline.is_some());
            }
            "N_devices" => {
                let row = evidence
                    .iter()
                    .find(|e| e.subject.name() == Some("Mobile"))
                    .unwrap();
                assert_eq!(row.facts["Conversions"], "18");
                assert_eq!(row.facts["Spend"], "$3,600");
                let desktop = evidence
                    .iter()
                    .find(|e| e.subject.name() == Some("Desktop"))
                    .unwrap();
                assert_eq!(desktop.facts["Spend"], "$400");
                assert_eq!(desktop.facts["Conversions"], "20");
                assert!(row.baseline.is_none());
                assert!(facts.contains("Mobile"));
                assert!(facts.contains("$3,600"));
                assert!(facts.contains("no_baseline"));
            }
            "O_spend" => assert!(steps.iter().any(|s| matches!(
                s.operation,
                Operation::AccountOverview {
                    period: Period::Last7Days,
                    ..
                }
            ) && s.evidence[0].days.unwrap().end
                - s.evidence[0].days.unwrap().start
                == 6)),
            "P_funding" => {
                assert_eq!(scope.accounts().len(), 3);
                assert!(facts.contains("Alder Dental"));
                assert!(facts.contains("Birch Dental"));
                assert!(facts.contains("Maple Dental"));
                assert!(facts.contains("$100"));
                assert!(facts.contains("$8.00"));
                assert!(facts.contains("budget-limited"));
            }
            "Q_display" => {
                let d = &data[&Handle::new(0)];
                assert_eq!(
                    d.campaigns
                        .iter()
                        .find(|c| c.id == 211)
                        .unwrap()
                        .channel_type,
                    "DISPLAY"
                );
                let now = metrics::sum(&d.days, d.end_day - 29, d.end_day, Some(211));
                let before = metrics::sum(&d.days, d.end_day - 59, d.end_day - 30, Some(211));
                assert!((now.spend / before.spend - 2.0).abs() < 0.02);
                assert!(
                    (now.conversions.unwrap() / before.conversions.unwrap() - 1.0).abs() < 0.02
                );
                assert!(d
                    .deep_check
                    .as_ref()
                    .unwrap()
                    .data
                    .audit
                    .search_terms
                    .is_empty());
            }
            "R_small_drop" => {
                let d = &data[&Handle::new(0)];
                let now = metrics::sum(&d.days, d.end_day - 29, d.end_day, None);
                let before = metrics::sum(&d.days, d.end_day - 59, d.end_day - 30, None);
                assert!(
                    (now.conversions.unwrap() / before.conversions.unwrap() - 0.95).abs() < 0.01
                );
                let rows = run::run(
                    &Operation::InvestigateConversions {
                        account: Handle::new(0),
                    },
                    1,
                    &scope,
                    &data,
                )
                .unwrap();
                assert_eq!(rows[0].facts["Conversion tracking"], "working");
            }
            _ => panic!("scenario without an evidence assertion"),
        }
    }
}

#[test]
fn the_evaluation_counts_numbers_outside_cited_evidence_and_actions_on_weak_evidence() {
    let scenario = scenarios::all().remove(0);
    let (scope, data) = build(&scenario);
    let steps = precompute(&scope, &data, scenario.period);
    let evidence = steps
        .iter()
        .flat_map(|s| s.evidence.clone())
        .collect::<Vec<_>>();
    let weak = evidence
        .iter()
        .find(|e| e.strength == Strength::Weak)
        .unwrap();
    let report = json!({"summary":null,"observations":[{"text":"Spend was $999999999.","evidence":[weak.id]}],
        "recommendations":[{"kind":"act","action":"Raise budget.","evidence":[weak.id]}]});
    let measured = metrics(&report, &evidence, &steps, &scenario, true);
    assert_eq!(measured["unsupported_numbers"], json!(["999999999"]));
    assert_eq!(measured["weak_actions"], 1);
    let report =
        json!({"summary":null,"observations":[{"text":"Spend was $999999999.","evidence":[]}]});
    assert_eq!(
        metrics(&report, &evidence, &steps, &scenario, true)["unsupported_numbers"],
        json!(["999999999"])
    );
}

#[test]
#[ignore = "recomputes metrics on KEYSEND_EVAL_DIR without model calls"]
fn the_evaluation_recomputes_the_saved_reports() {
    let directory =
        std::path::PathBuf::from(std::env::var("KEYSEND_EVAL_DIR").expect("KEYSEND_EVAL_DIR"));
    for path in std::fs::read_dir(directory).unwrap() {
        let path = path.unwrap().path();
        if path.extension().and_then(|s| s.to_str()) != Some("json")
            || path.to_string_lossy().ends_with(".judged.json")
        {
            continue;
        }
        let mut result: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let Some(arm) = result["arm"].as_str() else {
            continue;
        };
        let scenario = scenarios::all()
            .into_iter()
            .find(|s| result["scenario"] == s.key)
            .unwrap();
        let evidence = serde_json::from_value::<Vec<Evidence>>(result["evidence"].clone()).unwrap();
        let steps =
            serde_json::from_value::<Vec<InvestigationStep>>(result["steps"].clone()).unwrap();
        result["metrics"] = metrics(
            &result["report"],
            &evidence,
            &steps,
            &scenario,
            arm == "unchecked",
        );
        std::fs::write(path, serde_json::to_string_pretty(&result).unwrap()).unwrap();
    }
}

#[test]
fn an_ad_account_handle_in_a_recommendation_target_is_not_an_invented_metric() {
    let scenario = scenarios::all().remove(0);
    let (scope, data) = build(&scenario);
    let steps = precompute(&scope, &data, scenario.period);
    let evidence = steps
        .iter()
        .flat_map(|s| s.evidence.clone())
        .collect::<Vec<_>>();
    let report = json!({"recommendations":[{"kind":"investigate","target":"A1","action":"Review tracking.","evidence":[evidence[0].id]}]});
    assert_eq!(
        metrics(&report, &evidence, &steps, &scenario, true)["unsupported_numbers"],
        json!([])
    );
    let report = json!({"recommendations":[{"kind":"investigate","target":"A999999","action":"Review tracking.","evidence":[evidence[0].id]}]});
    assert_eq!(
        metrics(&report, &evidence, &steps, &scenario, true)["unsupported_numbers"],
        json!(["999999"])
    );
}

#[test]
#[ignore = "checks the saved drafts of KEYSEND_EVAL_DIR again with the current check, without model calls"]
fn the_evaluation_checks_saved_drafts_again() {
    let directory =
        std::path::PathBuf::from(std::env::var("KEYSEND_EVAL_DIR").expect("KEYSEND_EVAL_DIR"));
    let mut totals = (0, 0, 0, 0);
    for path in std::fs::read_dir(directory).unwrap() {
        let path = path.unwrap().path();
        let name = path.to_string_lossy().to_string();
        if !name.ends_with(".agent.json") {
            continue;
        }
        let result: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        if result["draft"].is_null() {
            continue;
        }
        let scenario = scenarios::all()
            .into_iter()
            .find(|s| result["scenario"] == s.key)
            .unwrap();
        let (scope, data) = build(&scenario);
        let names = data
            .values()
            .flat_map(|d| {
                std::iter::once(d.name.clone()).chain(d.campaigns.iter().map(|c| c.name.clone()))
            })
            .collect::<Vec<_>>();
        let names = names.iter().map(String::as_str).collect::<Vec<_>>();
        let context = json!({
            "period": match scenario.period { Period::Last7Days => "last 7 days", Period::Last30Days => "last 30 days" },
            "ad_accounts": scope.accounts().iter().map(|a| &a.name).collect::<Vec<_>>(),
            "terms": investigation::TERMS,
        });
        let evidence = serde_json::from_value::<Vec<Evidence>>(result["evidence"].clone()).unwrap();
        let draft = serde_json::from_value(result["draft"].clone()).unwrap();
        let report = super::answer::check(draft, &evidence, &names, &context);
        let before = result["report"]["recommendations"]
            .as_array()
            .map_or(0, Vec::len);
        totals.0 += before;
        totals.1 += report.recommendations.len();
        totals.2 += result["report"]["refused"].as_u64().unwrap_or(0) as usize;
        totals.3 += report.refused;
        if before != report.recommendations.len() {
            println!(
                "{name}: recommendations {before} -> {}",
                report.recommendations.len()
            );
        }
    }
    println!(
        "recommendations kept {} -> {}; texts refused {} -> {}",
        totals.0, totals.1, totals.2, totals.3
    );
}
