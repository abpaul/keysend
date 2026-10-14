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

const BUDGET: [(&str, usize, usize); 13] = [
    ("/ad-accounts", 5, 2598),
    ("/ad-accounts/4829153307", 19, 108573),
    ("/ad-accounts/4829153307?window=30", 19, 110155),
    ("/ad-accounts/4829153307/campaigns/701", 22, 82189),
    ("/ad-accounts/4829153307/settings", 11, 5787),
    ("/ad-accounts/4829153307/campaigns/701/settings", 9, 4759),
    ("/subscription", 4, 2352),
    ("/ad-accounts/4829153307/export.pdf?window=7", 24, 79336),
    (
        "/ad-accounts/4829153307/campaigns/701/export.pdf?window=7",
        23,
        101264,
    ),
    ("/", 0, 83432),
    ("/for/business-owners", 0, 23406),
    ("/for/agency-clients", 0, 62488),
    ("/for/marketing-managers", 0, 14825),
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
                    changes: vec![],
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
    f
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
    let mut times = Vec::new();
    let mut statements = Vec::new();
    let mut bytes = 0;
    for _ in 0..REQUESTS {
        f.statements.lock().unwrap().clear();
        let started = Instant::now();
        let response = f.request.get(path).add_cookie(f.a.clone()).await;
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
