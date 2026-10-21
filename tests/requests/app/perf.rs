use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use chrono::{Days, NaiveDate};
use keysend::{
    monitoring::{
        audit::{Audit, AuditRow, Counts, Data},
        campaign_settings::{CampaignSettings, Choice},
        deep_check::{DeepCheck, Finding},
        detection::{CampaignMovement, Detection},
        findings,
        metrics::Window,
        update::{CampaignLine, Update},
    },
    workspace::{
        history::{day_number, AccountChange, Campaign, CampaignDay},
        monitoring::{
            ActionDay, BreakdownDays, DeepBreakdowns, DemographicTotal, HourTotal, KeywordQuality,
            LocationTotal, NetworkDay, Recommendation, RecommendationState, SearchShareDay, Share,
        },
        AdAccountChoice, WorkspaceStore,
    },
};
use sea_orm::ConnectionTrait;
use serial_test::serial;

use super::*;

const ACCOUNTS: [&str; 3] = ["4829153307", "4829153308", "4829153309"];
const CAMPAIGNS: i64 = 20;
const HISTORY_DAYS: u64 = 730;
const UPDATES: u64 = 31;
const REQUESTS: usize = 5;
const TOLERANCE_PERCENT: usize = 10;

const BUDGET: [(&str, usize, usize); 17] = [
    ("/investigations", 6, 5720),
    ("/investigations/1", 6, 8142),
    ("/ad-accounts", 10, 3873),
    ("/ad-accounts/4829153307", 19, 108573),
    ("/ad-accounts/4829153307?window=30", 19, 110155),
    ("/ad-accounts/4829153307/campaigns/701", 22, 82189),
    ("/ad-accounts/4829153307/settings", 14, 8731),
    ("/ad-accounts/4829153307/campaigns/701/settings", 9, 5665),
    ("/subscription", 4, 3584),
    ("/optimize", 11, 9652),
    ("/optimize/activity", 5, 7649),
    ("/ad-accounts/4829153307/export.pdf?window=7", 28, 187503),
    (
        "/ad-accounts/4829153307/campaigns/701/export.pdf?window=7",
        25,
        141118,
    ),
    ("/", 0, 83432),
    ("/for/business-owners", 0, 23406),
    ("/for/agency-clients", 0, 62488),
    ("/for/marketing-managers", 0, 16682),
];

fn end_date() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 11).unwrap()
}

fn campaigns() -> Vec<Campaign> {
    (1..=CAMPAIGNS)
        .map(|index| Campaign {
            id: 700 + index,
            name: format!("Search - Campaign {index:02}"),
            status: "ENABLED".into(),
            channel_type: "SEARCH".into(),
            bidding_strategy: "MAXIMIZE_CONVERSIONS".into(),
            budget_micros: Some(80_000_000),
        })
        .collect()
}

fn days() -> Vec<CampaignDay> {
    let mut rows = Vec::new();
    for offset in 0..HISTORY_DAYS {
        let day = end_date().checked_sub_days(Days::new(offset)).unwrap();
        for index in 1..=CAMPAIGNS {
            for device in 1..=2_i64 {
                let wave = ((offset as i64 * 7 + index * 13 + device * 5) % 40) as f64;
                rows.push(CampaignDay {
                    day,
                    campaign_id: 700 + index,
                    device,
                    counts: Counts {
                        impressions: 800.0 + wave * 10.0,
                        clicks: 40.0 + wave,
                        cost: 30.0 + wave,
                        conversions: 2.0 + (wave / 10.0).floor(),
                        value: 100.0 + wave * 3.0,
                    },
                });
            }
        }
    }
    rows
}

fn changes(ad: &str) -> Vec<AccountChange> {
    (0..12)
        .map(|offset| {
            let day = end_date().checked_sub_days(Days::new(offset * 5)).unwrap();
            AccountChange {
                changed_at: format!("{day} 10:00:00"),
                resource_name: format!("customers/{ad}/changeEvents/{offset}"),
                change_resource_name: None,
                resource_type: "CAMPAIGN_BUDGET".into(),
                operation: "UPDATE".into(),
                campaign_id: Some(701 + offset as i64),
                changed_fields: String::new(),
                old_value: r#"{"campaignBudget":{"amountMicros":"50000000"}}"#.into(),
                new_value: r#"{"campaignBudget":{"amountMicros":"80000000"}}"#.into(),
            }
        })
        .collect()
}

fn audit_rows(prefix: &str) -> Vec<AuditRow> {
    (1..=5)
        .map(|index| AuditRow {
            name: format!("{prefix} {index}"),
            counts: Counts {
                impressions: 900.0,
                clicks: 60.0 - index as f64,
                conversions: index as f64,
                cost: 100.0 + index as f64,
                value: 0.0,
            },
            ..Default::default()
        })
        .collect()
}

fn ads() -> Vec<AuditRow> {
    [
        (
            "Emergency Dentist | Open Today | Harbour Dental",
            Some(9001),
            134.0,
            720.0,
        ),
        (
            "Responsive display ad in ad group Emergency",
            Some(9002),
            58.0,
            4100.0,
        ),
        (
            "Same-Day Appointments | Call Harbour Dental",
            Some(9003),
            0.0,
            0.0,
        ),
        ("Not available (9004)", None, 41.0, 900.0),
    ]
    .into_iter()
    .zip(["GOOD", "", "POOR", "Not available"])
    .map(|((name, id, clicks, impressions), strength)| AuditRow {
        name: name.into(),
        id,
        counts: Counts {
            impressions,
            clicks,
            conversions: (clicks / 20.0).floor(),
            cost: clicks * 2.0,
            value: 0.0,
        },
        strength: strength.into(),
        ..Default::default()
    })
    .collect()
}

fn campaign_row(id: i64) -> AuditRow {
    AuditRow {
        name: format!("Search - Campaign {:02}", id - 700),
        id: Some(id),
        counts: Counts {
            impressions: 9000.0,
            clicks: 600.0,
            conversions: 30.0,
            cost: 1050.0,
            value: 0.0,
        },
        ..Default::default()
    }
}

fn deep_check(day: i64) -> (DeepCheck, Vec<Recommendation>) {
    let by_campaign = (1..=CAMPAIGNS)
        .map(|index| {
            (
                700 + index,
                Audit {
                    conversion_actions: Some(1),
                    campaigns: vec![campaign_row(700 + index)],
                    search_terms: audit_rows("term"),
                    keywords: audit_rows("keyword"),
                    landing_pages: audit_rows("page"),
                    ads: ads(),
                    ..Default::default()
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    let budget = Finding {
        campaign_id: Some(702),
        rule: findings::Finding {
            key: "budget-limited:702:Search - Campaign 02 (702)".into(),
            title: "Search - Campaign 02 is limited by its budget".into(),
            detail: "Search - Campaign 02 lost 20% of its search impressions to budget. Its cost per conversion is $12.00, at or below the Ad Account value of $15.00, with 61 conversions.".into(),
            subjects: vec!["Search - Campaign 02".into()],
            spend: 400.0,
        },
    };
    let findings = (1..=7)
        .map(|index| Finding {
            campaign_id: Some(700 + index),
            rule: findings::Finding {
                key: format!("wasted-term:{}:term {index}", 700 + index),
                title: "Wasted search term".into(),
                detail: format!("term {index}: $186 spend, 74 clicks, 0 conversions."),
                subjects: vec![format!("term {index}")],
                spend: 186.0 + index as f64,
            },
        })
        .chain([budget])
        .collect::<Vec<_>>();
    let recommendations = findings
        .iter()
        .map(|finding| Recommendation {
            finding_key: finding.rule.key.clone(),
            action: if finding.rule.key.starts_with("budget-limited") {
                format!("Increase the daily budget of {}.", finding.rule.subjects[0])
            } else {
                format!("Add {} as a negative keyword.", finding.rule.subjects[0])
            },
            reason: "It spends without conversions.".into(),
            evidence: finding.rule.detail.clone(),
            measure: String::new(),
            intent: String::new(),
            subject: finding.rule.subjects[0].clone(),
            confidence: keysend::monitoring::recommendation::Confidence::High,
            campaign_id: finding.campaign_id,
            state: RecommendationState::Open,
        })
        .collect();
    (
        DeepCheck {
            day,
            data: Data {
                currency: "USD".into(),
                reporting: Counts {
                    impressions: 90_000.0,
                    clicks: 6_000.0,
                    cost: 9_000.0,
                    conversions: 600.0,
                    value: 30_000.0,
                },
                audit: Audit {
                    conversion_actions: Some(1),
                    campaigns: audit_rows("Campaign"),
                    ..Default::default()
                },
            },
            by_campaign,
            findings,
            judge_by_conversions: true,
            recommendations_generated: true,
            assessment: None,
            research: None,
        },
        recommendations,
    )
}

async fn seeded() -> Fixture {
    let f = Fixture::monitoring().await;
    let signed = identity::authenticate(&f.ctx.db, f.a.value())
        .await
        .unwrap()
        .unwrap();
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let connection = store.google_connections().await.unwrap().remove(0);
    let selection = store
        .create_selection(
            &connection.id,
            ACCOUNTS[1..]
                .iter()
                .map(|id| AdAccountChoice {
                    customer_id: (*id).into(),
                    name: format!("Account {id}"),
                    login_customer_id: None,
                })
                .collect(),
        )
        .await
        .unwrap();
    assert!(store
        .save_selected(
            &selection,
            &ACCOUNTS[1..]
                .iter()
                .map(|id| (*id).to_owned())
                .collect::<Vec<_>>()
        )
        .await
        .unwrap());
    let today = day_number(end_date()) + 1;
    for ad in ACCOUNTS {
        store
            .save_ad_account_metadata(ad, "USD", "UTC")
            .await
            .unwrap();
        let from = end_date()
            .checked_sub_days(Days::new(HISTORY_DAYS - 1))
            .unwrap();
        store
            .save_history(ad, from, end_date(), &campaigns(), &days(), &changes(ad))
            .await
            .unwrap();
        for offset in 0..UPDATES {
            let day = today - offset as i64;
            let metrics = store
                .window_metrics(ad, day - 1, Window::SevenDays, None)
                .await
                .unwrap();
            let lines = (1..=5_i64)
                .map(|index| CampaignLine {
                    campaign_id: 700 + index,
                    text: format!("Spend held steady at the usual level on day {offset}."),
                    cause: None,
                })
                .collect::<Vec<_>>();
            store
                .save_update(
                    ad,
                    &Update {
                        day,
                        summary: format!(
                            "In the last 7 days, spend was $5,000 against $4,800 on day {offset}."
                        ),
                        highlight: None,
                        cause: None,
                        lines: lines.clone(),
                        metrics: metrics.clone(),
                        detection: Detection {
                            day,
                            currency: "USD".into(),
                            metrics: metrics.clone(),
                            movements: vec![],
                            yesterday: vec![],
                            campaigns: lines
                                .iter()
                                .map(|line| CampaignMovement {
                                    campaign_id: line.campaign_id,
                                    name: format!(
                                        "Search - Campaign {:02}",
                                        line.campaign_id - 700
                                    ),
                                    metrics: metrics.clone(),
                                    movements: vec![],
                                    reasons: vec![],
                                    account_changes: vec![],
                                    follow_ups: vec![],
                                })
                                .collect(),
                            judge_by_conversions: true,
                            causes: vec![],
                        },
                        not_useful: false,
                        written_without_ai: true,
                        more_campaigns: 0,
                    },
                )
                .await
                .unwrap();
        }
        let (check, recommendations) = deep_check(today - 1);
        store
            .refresh_deep_check(ad, &check, &recommendations)
            .await
            .unwrap();
    }
    seed_investigations(&f).await;
    seed_optimize(&f).await;
    f
}

/// Optimize as the production evaluation leaves it: the first Ad Account recommends cuts,
/// the second runs an Autopilot that applied one cut, the third records Shadow decisions.
async fn seed_optimize(f: &Fixture) {
    use keysend::optimize::{
        ads::Simulated,
        cycle, execute,
        policy::{Live, Policy, Standing, AUTOMATIC_KINDS},
        propose::Budget,
        Mode, ProposalState,
    };
    let signed = identity::authenticate(&f.ctx.db, f.a.value())
        .await
        .unwrap()
        .unwrap();
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let mut settings = keysend::settings::Settings::load(&f.ctx).unwrap();
    settings.optimize.live = true;
    settings.optimize.max_mode = Mode::Autopilot;
    let now = settings.now().timestamp();
    let budgets = campaigns()
        .iter()
        .map(|campaign| {
            (
                campaign.id,
                Budget {
                    budget_id: campaign.id + 8300,
                    shared: campaign.id == 702,
                    campaigns: if campaign.id == 702 { 3 } else { 1 },
                },
            )
        })
        .collect::<Vec<_>>();
    for (ad, mode) in [
        (ACCOUNTS[0], Mode::Recommend),
        (ACCOUNTS[1], Mode::Autopilot),
        (ACCOUNTS[2], Mode::Shadow),
    ] {
        store.save_campaign_budgets(ad, &budgets).await.unwrap();
        let policy = Policy {
            mode,
            automatic: if mode == Mode::Autopilot {
                AUTOMATIC_KINDS.into_iter().collect()
            } else {
                Default::default()
            },
            envelope_micros: Some(2_000_000_000),
            max_daily_increase_micros: 50_000_000,
            target_cost_micros: Some(10_000_000),
            ..Default::default()
        };
        store
            .save_optimization_policy(ad, &policy, Standing::Active, "user", now - 86_400)
            .await
            .unwrap();
        let authorized = cycle::evaluate(&settings, &store, ad, now).await.unwrap();
        assert!(
            authorized.len() <= Policy::default().changes_per_day as usize,
            "the Autopilot authorized more changes than the day allows"
        );
        if let Some(first) = authorized.first() {
            let proposal = store.proposal(*first).await.unwrap().unwrap();
            let campaign = proposal.draft.action.campaign_id();
            let google = Simulated::new(vec![Live {
                campaign_id: campaign,
                status: "ENABLED".into(),
                budget_id: Some(campaign + 8300),
                budget_micros: Some(80_000_000),
                budget_shared: false,
                budget_campaigns: 1,
                bidding_strategy: "MAXIMIZE_CONVERSIONS".into(),
            }]);
            let day = day_number(end_date()) + 1;
            execute::run(&settings, &store, &google, *first, day, 1_600_000_000, now)
                .await
                .unwrap();
            assert_eq!(
                store.proposal(*first).await.unwrap().unwrap().state,
                ProposalState::Applied
            );
        }
    }
    assert!(!store
        .proposals(Some(ACCOUNTS[0]), &[ProposalState::Open], 10)
        .await
        .unwrap()
        .is_empty());
}

async fn seed_investigations(f: &Fixture) {
    use keysend::agent::{
        answer::{self, Draft},
        data,
        investigation::Reply,
        operation::{Operation, Period},
        Failure, State,
    };
    use keysend::workspace::investigations::{
        InvestigationFinish, InvestigationStep, InvestigationUsage, Outcome,
    };
    let signed = identity::authenticate(&f.ctx.db, f.a.value())
        .await
        .unwrap()
        .unwrap();
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = keysend::settings::Settings::load(&f.ctx)
        .unwrap()
        .now()
        .timestamp();
    let selected = vec![ACCOUNTS[0].to_owned()];
    let scope = data::Scope::load(&store, &selected).await.unwrap();
    let snapshots = data::load_scope(&store, &scope, day_number(end_date()))
        .await
        .unwrap();
    let operation: Operation = serde_json::from_value(json!({"operation":"compare_campaigns","account":"A1","period":"last_30_days","order":"spend","rows":5})).unwrap();
    let evidence = keysend::agent::run::run(&operation, 1, &scope, &snapshots).unwrap();
    let name = evidence[1].subject.name().unwrap();
    let spend = evidence[1].facts["Spend"].as_str().unwrap();
    let draft: Draft = serde_json::from_value(json!({
        "summary":format!("{name} spent {spend}. Check conversion tracking before changing its budget."),
        "observations":[{"text":format!("{name} spent {spend}."),"evidence":["E1.2"]}],
        "hypotheses":[{"text":"Conversion delay may affect the recent results.","evidence":["E1.2"]}],
        "recommendations":[{"kind":"investigate","action":format!("Review {name}."),"target":name,"evidence":["E1.2"],"rationale":"Conversion tracking may need review.","risk":"Changing the budget before reviewing tracking may hide the problem.","measure":"Compare conversions and cost per conversion."}],"limitations":[]
    })).unwrap();
    let report = answer::check(
        draft,
        &evidence,
        &[name],
        &serde_json::json!({"period": "last 30 days"}),
    );
    assert!(!report.observations.is_empty());
    let id = store
        .create_investigation(
            "Which campaigns need attention before I change their budgets?",
            &selected,
            Period::Last30Days,
            None,
            now - 1800,
            i64::MAX,
            0,
        )
        .await
        .unwrap()
        .unwrap();
    store.claim_investigation(id, now - 1800).await.unwrap();
    store
        .save_investigation_step(
            id,
            &InvestigationStep {
                ordinal: 1,
                round: 0,
                operation,
                outcome: Outcome::Answered,
                evidence,
                elapsed_ms: 20,
            },
        )
        .await
        .unwrap();
    store
        .set_investigation_state(id, State::Answering)
        .await
        .unwrap();
    store
        .finish_investigation(
            id,
            &InvestigationFinish {
                state: State::Completed,
                failure: None,
                report: Some(serde_json::to_value(Reply::Answer(report)).unwrap()),
                usage: InvestigationUsage {
                    model_calls: 2,
                    input_tokens: 800,
                    output_tokens: 250,
                },
            },
            now - 1780,
        )
        .await
        .unwrap();
    let id = store
        .create_investigation(
            "Why did conversions drop?",
            &selected,
            Period::Last7Days,
            None,
            now - 1200,
            i64::MAX,
            0,
        )
        .await
        .unwrap()
        .unwrap();
    store.claim_investigation(id, now - 1200).await.unwrap();
    store
        .set_investigation_state(id, State::Answering)
        .await
        .unwrap();
    store
        .finish_investigation(
            id,
            &InvestigationFinish {
                state: State::Completed,
                failure: None,
                report: Some(
                    serde_json::to_value(Reply::Clarification {
                        question: "Which Campaign should we investigate?".into(),
                    })
                    .unwrap(),
                ),
                usage: InvestigationUsage::default(),
            },
            now - 1190,
        )
        .await
        .unwrap();
    let id = store
        .create_investigation(
            "Where might we be wasting budget?",
            &selected,
            Period::Last30Days,
            None,
            now - 600,
            i64::MAX,
            0,
        )
        .await
        .unwrap()
        .unwrap();
    store
        .finish_investigation(
            id,
            &InvestigationFinish {
                state: State::Failed,
                failure: Some(Failure::Model),
                report: None,
                usage: InvestigationUsage::default(),
            },
            now - 590,
        )
        .await
        .unwrap();
}

async fn seed_breakdowns(f: &Fixture) {
    let signed = identity::authenticate(&f.ctx.db, f.a.value())
        .await
        .unwrap()
        .unwrap();
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let ad = ACCOUNTS[0];
    let last = day_number(end_date());
    let (first, deep_day) = (last - 55, last);
    let mut shares = Vec::new();
    let mut networks = Vec::new();
    let mut actions = Vec::new();
    for offset in 0..56_i64 {
        for index in 1..=CAMPAIGNS {
            let wave = ((offset * 3 + index * 7) % 20) as f64 / 100.0;
            shares.push(SearchShareDay {
                day: last - offset,
                campaign_id: 700 + index,
                impressions: 900,
                shown: Share::Exact(0.5 + wave),
                lost_budget: Share::Exact(0.3 - wave / 2.0),
                lost_rank: Share::Exact(0.2 - wave / 2.0),
            });
            for (network, cost, clicks) in [("SEARCH", 24.0, 36.0), ("SEARCH_PARTNERS", 6.0, 4.0)] {
                networks.push(NetworkDay {
                    day: last - offset,
                    campaign_id: 700 + index,
                    network: network.into(),
                    counts: Counts {
                        impressions: 500.0,
                        clicks,
                        cost,
                        conversions: clicks / 12.0,
                        value: 0.0,
                    },
                });
            }
            for (action, conversions) in [
                ("Booking form", 2.0),
                ("Phone call", 1.0),
                ("Directions", 0.5),
            ] {
                actions.push(ActionDay {
                    day: last - offset,
                    campaign_id: 700 + index,
                    action: action.into(),
                    conversions,
                    value: 0.0,
                });
            }
        }
    }
    store
        .save_breakdown(ad, first, last, BreakdownDays::SearchShare(shares))
        .await
        .unwrap();
    store
        .save_breakdown(ad, first, last, BreakdownDays::Networks(networks))
        .await
        .unwrap();
    store
        .save_breakdown(ad, first, last, BreakdownDays::ConversionActions(actions))
        .await
        .unwrap();

    let (mut check, recommendations) = deep_check(deep_day);
    let row = |name: &str, cost: f64, clicks: f64, conversions: f64| AuditRow {
        name: name.into(),
        counts: Counts {
            impressions: clicks * 12.0,
            clicks,
            conversions,
            cost,
            value: 0.0,
        },
        ..Default::default()
    };
    let first = check.by_campaign.get_mut(&701).unwrap();
    first.search_terms = vec![
        row("emergency dentist near me", 310.0, 190.0, 14.0),
        row("emergency dentist", 240.0, 150.0, 9.0),
        row("free dental care", 96.0, 61.0, 0.0),
        row("dentist jobs harbour city", 41.0, 22.0, 0.0),
        row("24 hour dentist open now", 38.0, 19.0, 2.0),
    ];
    first.keywords = vec![
        row("emergency dentist", 520.0, 300.0, 21.0),
        row("dentist open now", 260.0, 120.0, 4.0),
        row("tooth pain", 180.0, 110.0, 0.0),
        row("24 hour dentist", 90.0, 70.0, 5.0),
    ];
    first.landing_pages = vec![
        row("https://harbourdental.example/emergency-dentist-open-now/same-day-appointments?utm_source=google&utm_medium=cpc&utm_campaign=emergency", 610.0, 350.0, 24.0),
        row("https://harbourdental.example/", 380.0, 240.0, 6.0),
        row("https://harbourdental.example/prices", 60.0, 0.0, 0.0),
    ];
    assert!(store
        .save_campaign_settings(
            ad,
            702,
            &CampaignSettings {
                judge_by_conversions: Choice::No,
                ..Default::default()
            },
        )
        .await
        .unwrap());
    f.ctx
        .db
        .execute_unprepared(&format!(
            "UPDATE deep_checks SET day=day-7 WHERE ad_account_customer_id='{ad}'"
        ))
        .await
        .unwrap();
    let totals = |clicks: f64, cost: f64, conversions: f64| Counts {
        impressions: clicks * 10.0,
        clicks,
        cost,
        conversions,
        value: 0.0,
    };
    let mut hours = Vec::new();
    for weekday in 0..7_i64 {
        for hour in 0..24_i64 {
            let busy = (8..18).contains(&hour) && weekday < 5;
            let clicks = if busy {
                20.0 + (hour % 5) as f64 * 6.0
            } else {
                (hour % 3) as f64
            };
            hours.push(HourTotal {
                weekday,
                hour,
                counts: totals(clicks, clicks * 2.0, clicks / 8.0),
            });
        }
    }
    let demographic = |rows: &[(&str, f64)]| {
        rows.iter()
            .map(|(value, clicks)| DemographicTotal {
                value: (*value).into(),
                counts: totals(*clicks, clicks * 2.0, clicks / 10.0),
            })
            .collect::<Vec<_>>()
    };
    let keyword =
        |criterion_id: i64, keyword: &str, match_type: &str, score: Option<i64>| KeywordQuality {
            campaign_id: 701,
            ad_group_id: 55,
            criterion_id,
            keyword: keyword.into(),
            match_type: match_type.into(),
            cost: 1000.0 - criterion_id as f64,
            score,
            ad_relevance: score.map(|_| "AVERAGE".into()),
            landing_page: score.map(|_| "ABOVE_AVERAGE".into()),
            expected_ctr: score.map(|_| "BELOW_AVERAGE".into()),
        };
    let rows = |keywords: Vec<KeywordQuality>| DeepBreakdowns {
        hours: Ok(hours.clone()),
        locations: Ok((1..=10)
            .map(|index| LocationTotal {
                id: 1_000_000 + index,
                name: format!("Harbour District {index}, New South Wales"),
                counts: totals(
                    400.0 - index as f64 * 30.0,
                    900.0 - index as f64 * 60.0,
                    30.0 - index as f64 * 2.0,
                ),
            })
            .collect()),
        keywords: Ok(keywords),
        ages: Ok(demographic(&[
            ("AGE_RANGE_18_24", 80.0),
            ("AGE_RANGE_25_34", 500.0),
            ("AGE_RANGE_35_44", 400.0),
            ("AGE_RANGE_45_54", 300.0),
            ("AGE_RANGE_55_64", 120.0),
            ("AGE_RANGE_UNDETERMINED", 60.0),
        ])),
        genders: Ok(demographic(&[
            ("FEMALE", 700.0),
            ("MALE", 600.0),
            ("UNDETERMINED", 100.0),
        ])),
        delay: Some(Ok(vec![
            ("LESS_THAN_ONE_DAY".into(), 150.0),
            ("THREE_TO_FOUR_DAYS".into(), 50.0),
        ])),
    };
    store
        .save_deep_breakdowns(
            ad,
            deep_day - 7,
            &rows(vec![
                keyword(901, "emergency dentist", "EXACT", Some(7)),
                keyword(904, "24 hour dentist", "PHRASE", Some(8)),
            ]),
        )
        .await
        .unwrap();
    store
        .save_deep_breakdowns(
            ad,
            deep_day,
            &rows(vec![
                keyword(901, "emergency dentist", "EXACT", Some(5)),
                keyword(902, "dentist open now", "PHRASE", Some(3)),
                keyword(903, "tooth pain", "BROAD", None),
                keyword(904, "24 hour dentist", "PHRASE", Some(8)),
            ]),
        )
        .await
        .unwrap();
    store
        .refresh_deep_check(ad, &check, &recommendations)
        .await
        .unwrap();
}

struct Measured {
    statements: Vec<String>,
    bytes: usize,
    median: Duration,
}

async fn measure(f: &Fixture, path: &str) -> Measured {
    measure_with_cookie(f, path, &f.a).await
}

async fn measure_with_cookie(
    f: &Fixture,
    path: &str,
    cookie: &axum_extra::extract::cookie::Cookie<'static>,
) -> Measured {
    let mut times = Vec::new();
    let mut statements = Vec::new();
    let mut bytes = 0;
    for _ in 0..REQUESTS {
        f.statements.lock().unwrap().clear();
        let started = Instant::now();
        let response = f.request.get(path).add_cookie(cookie.clone()).await;
        times.push(started.elapsed());
        assert_eq!(response.status_code(), 200, "{path}");
        bytes = response.as_bytes().len();
        statements = std::mem::take(&mut *f.statements.lock().unwrap());
    }
    times.sort();
    Measured {
        statements,
        bytes,
        median: times[REQUESTS / 2],
    }
}

fn over(measured: usize, committed: usize) -> bool {
    measured * 100 > committed * (100 + TOLERANCE_PERCENT)
}

#[tokio::test]
#[serial]
async fn pages_stay_inside_their_committed_query_and_byte_budget() {
    let f = seeded().await;
    let mut failures = Vec::new();
    println!(
        "\n{:<48}{:>9}{:>10}{:>11}",
        "page", "queries", "bytes", "median ms"
    );
    for (path, queries, bytes) in BUDGET {
        let measured = measure(&f, path).await;
        println!(
            "{path:<48}{:>9}{:>10}{:>11.1}",
            measured.statements.len(),
            measured.bytes,
            measured.median.as_secs_f64() * 1000.0
        );
        if std::env::var("KEYSEND_PERF_STATEMENTS").is_ok() {
            let mut repeated = BTreeMap::<&str, usize>::new();
            for statement in &measured.statements {
                *repeated.entry(statement).or_default() += 1;
            }
            let mut repeated = repeated.into_iter().collect::<Vec<_>>();
            repeated.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
            for (statement, count) in repeated.into_iter().take(12) {
                println!(
                    "    {count:>4} x {}",
                    statement.chars().take(110).collect::<String>()
                );
            }
        }
        if over(measured.statements.len(), queries) {
            failures.push(format!(
                "{path}: {} statements against {queries}",
                measured.statements.len()
            ));
        }
        if over(measured.bytes, bytes) {
            failures.push(format!("{path}: {} bytes against {bytes}", measured.bytes));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
#[serial]
async fn a_non_english_reader_costs_at_most_one_more_statement_for_a_page() {
    let f = seeded().await;
    let mut failures = Vec::new();
    let mut english = Vec::new();
    for (path, _, _) in BUDGET {
        english.push(measure(&f, path).await.statements.len());
    }
    f.ctx
        .db
        .execute_unprepared("UPDATE users SET language='ro' WHERE email='a@example.com'")
        .await
        .unwrap();
    let signed = identity::authenticate(&f.ctx.db, f.a.value())
        .await
        .unwrap()
        .unwrap();
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = keysend::settings::Settings::load(&f.ctx).unwrap().now();
    for ad in ACCOUNTS {
        let translated = keysend::translation::collect::shown(&store, ad, now)
            .await
            .unwrap()
            .iter()
            .map(|item| {
                (
                    keysend::translation::hash(&item.text),
                    format!("[ro] {}", item.text),
                )
            })
            .collect::<Vec<_>>();
        store
            .save_translations(
                ad,
                keysend::language::Language::Ro,
                &translated,
                now.timestamp(),
            )
            .await
            .unwrap();
    }
    for ((path, _, _), english) in BUDGET.iter().zip(english) {
        let romanian = measure(&f, path).await.statements;
        println!("{path:<48}{english:>4} -> {:>2}", romanian.len());
        if romanian.len() > english + 1 {
            failures.push(format!("{path}: {} against {english}", romanian.len()));
        }
        let reads = romanian
            .iter()
            .filter(|statement| statement.contains("FROM translations"))
            .count();
        if reads > 1 {
            failures.push(format!("{path}: {reads} reads of translations"));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
#[serial]
async fn the_first_translation_of_an_ad_account_asks_for_a_bounded_number_of_texts() {
    let f = seeded().await;
    let signed = identity::authenticate(&f.ctx.db, f.a.value())
        .await
        .unwrap()
        .unwrap();
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let settings = keysend::settings::Settings::load(&f.ctx).unwrap();
    let outcome = keysend::translation::translate_account(
        &settings,
        &store,
        ACCOUNTS[0],
        keysend::language::Language::Ro,
    )
    .await;
    {
        let calls = f.calls.lock().unwrap();
        let characters = calls
            .translations
            .iter()
            .map(|body| {
                body["input"].as_str().unwrap().len() + body["instructions"].as_str().unwrap().len()
            })
            .sum::<usize>();
        println!(
            "{outcome:?} requests {} characters {characters}",
            calls.translations.len()
        );
        assert!(outcome.missing <= keysend::translation::MAX_TEXTS);
        assert_eq!(outcome.requests, calls.translations.len());
        assert!(outcome.requests <= keysend::translation::MAX_TEXTS / 40 + 1);
        assert_eq!(outcome.rejected, 0);
    }
    let again = keysend::translation::translate_account(
        &settings,
        &store,
        ACCOUNTS[0],
        keysend::language::Language::Ro,
    )
    .await;
    assert!(again.missing < outcome.missing);
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
#[ignore = "serves pages for a manual browser check"]
async fn serve_the_seeded_pages_to_a_browser() {
    let directory = std::path::PathBuf::from(std::env::var("KEYSEND_BROWSER_CHECK_DIR").unwrap());
    let f = seeded().await;
    seed_breakdowns(&f).await;
    let signed = identity::authenticate(&f.ctx.db, f.a.value())
        .await
        .unwrap()
        .unwrap();
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let mut check = store.latest_deep_check(ACCOUNTS[0]).await.unwrap().unwrap();
    check.assessment = Some(keysend::monitoring::assessment::Assessment {
        verdict: keysend::monitoring::assessment::Verdict::Mixed,
        headline: "Search - Campaign 01 is bringing in more enquiries. Some searches still spend without a result.".into(),
        worked: vec![keysend::monitoring::claims::Point {
            observation: "Search - Campaign 01 delivered 42 conversions.".into(),
            explanation: Some("The longer opening hours may have helped more people enquire.".into()),
        }],
        attention: vec!["Search - Campaign 02 spent $186 on searches without conversions.".into()],
        next: vec!["Review the search terms in Search - Campaign 02.".into()],
        market: Some("Demand can vary through the autumn.".into()),
        written_without_ai: false,
    });
    check.research = Some(keysend::monitoring::business::Research {
        day: check.day,
        website: Some("https://example.com".into()),
        found: true,
        kind: keysend::monitoring::business::Kind::LocalService,
        summary: String::new(),
        offers: vec![],
        areas: vec![],
        facts: vec![keysend::monitoring::business::Fact {
            claim: "Demand can vary through the autumn.".into(),
            kind: keysend::monitoring::business::FactKind::Seasonality,
            source: "https://example.com/season".into(),
            verified: true,
        }],
    });
    store
        .save_profile(
            ACCOUNTS[0],
            &keysend::workspace::profile::Profile {
                economics: keysend::monitoring::economics::Economics {
                    margin: Some(0.35),
                    customer_value: Some(250.0),
                    close_rate: Some(0.2),
                    target_return: Some(4.0),
                },
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let mut recommendations = store
        .recommendations_with_findings(ACCOUNTS[0])
        .await
        .unwrap()
        .into_iter()
        .map(|found| found.recommendation)
        .collect::<Vec<_>>();
    for recommendation in &mut recommendations {
        recommendation.measure = "Check spend and conversions over the next 30 days.".into();
    }
    f.ctx.db.execute_raw(sea_orm::Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Sqlite,
        "DELETE FROM monitoring_recommendations WHERE workspace_id=? AND ad_account_customer_id=?",
        vec![signed.workspace.id().into(), ACCOUNTS[0].into()],
    )).await.unwrap();
    store
        .refresh_deep_check(ACCOUNTS[0], &check, &recommendations)
        .await
        .unwrap();
    f.ctx
        .db
        .execute_unprepared("DELETE FROM updates WHERE ad_account_customer_id='4829153309'")
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    std::fs::write(
        directory.join("address"),
        listener.local_addr().unwrap().to_string(),
    )
    .unwrap();
    std::fs::write(directory.join("cookie"), f.a.value()).unwrap();
    let router = f.router.clone();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    while !directory.join("stop").exists() {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test]
#[serial]
async fn admin_pages_stay_inside_their_committed_query_and_byte_budget() {
    let (f, cookie, id) = crate::requests::admin_pages::invented_dataset().await;
    let paths = [
        "/admin".to_owned(),
        "/admin/customers".to_owned(),
        format!("/admin/customers/{id}"),
        "/admin/revenue".to_owned(),
        "/admin/usage".to_owned(),
        "/admin/operations".to_owned(),
    ];
    let budgets = [
        (11, 13602),
        (3, 7245),
        (9, 7168),
        (6, 5891),
        (5, 5393),
        (8, 11124),
    ];
    let mut failures = Vec::new();
    for (path, (queries, bytes)) in paths.iter().zip(budgets) {
        let measured = measure_with_cookie(&f, path, &cookie).await;
        println!(
            "{path}: {} queries, {} bytes (five successful authenticated requests)",
            measured.statements.len(),
            measured.bytes
        );
        if over(measured.statements.len(), queries) || over(measured.bytes, bytes) {
            failures.push(format!(
                "{path}: {} queries / {} bytes",
                measured.statements.len(),
                measured.bytes
            ));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
