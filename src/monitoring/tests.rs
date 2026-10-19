use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::{
    access, alerts,
    audit::{Audit, AuditRow, Counts, Data},
    campaign_settings::{
        self, AccountDefaults, AllFollowAccount, CampaignSettings, Choice, Resolved,
    },
    causes::{self, Level},
    deep_check, detection,
    metrics::{self, CampaignDay, Metrics, Window},
    tracking::{self, ConversionTracking},
    update,
};
use crate::{
    billing::Subscription,
    language::Language,
    workspace::history::{AccountChange, Campaign},
};

fn metrics(spend: f64, clicks: f64, conversions: f64) -> Metrics {
    Metrics {
        spend,
        clicks,
        impressions: clicks * 10.0,
        conversions: Some(conversions),
        conversion_value: Some(conversions * 5.0),
    }
}
fn days(end: i64) -> Vec<CampaignDay> {
    (0..=end)
        .map(|day| CampaignDay {
            day,
            campaign_id: 701,
            metrics: metrics(10.0, 10.0, 1.0),
        })
        .collect()
}
fn campaign() -> Campaign {
    Campaign {
        id: 701,
        name: "Search - Real".into(),
        status: "ENABLED".into(),
        channel_type: "SEARCH".into(),
        bidding_strategy: "MAXIMIZE_CONVERSIONS".into(),
        budget_micros: Some(80_000_000),
    }
}
fn detection(days: &[CampaignDay], first: i64, judge: bool) -> detection::Detection {
    detection::detect(
        days,
        &[campaign()],
        &[],
        62,
        first,
        AllFollowAccount { judged: judge },
        "USD",
    )
}

#[test]
fn windows_have_equal_comparisons_and_keep_fractional_conversions() {
    let mut rows = days(62);
    for row in rows.iter_mut().filter(|row| row.day >= 56) {
        row.metrics = metrics(20.0, 30.0, 1.5);
    }
    let account = metrics::window_metrics(&rows, 62, Window::SevenDays, None, true);
    assert_eq!(account.current, metrics(140.0, 210.0, 10.5));
    assert_eq!(account.previous, metrics(70.0, 70.0, 7.0));
    let campaign = metrics::window_metrics(&rows, 62, Window::ThirtyDays, Some(701), true);
    assert_eq!(campaign.current.spend, 370.0);
    assert_eq!(campaign.previous.spend, 300.0);
    assert_eq!(
        metrics::window_metrics(&rows, 62, Window::SevenDays, Some(702), true)
            .current
            .conversions,
        Some(0.0)
    );
}

#[test]
fn conversion_metrics_are_left_out_when_results_cannot_be_judged() {
    let window = metrics::window_metrics(&days(62), 62, Window::SevenDays, None, false);
    assert_eq!(window.current.conversions, None);
    assert_eq!(window.previous.conversion_value, None);
    assert!(!serde_json::to_string(&window)
        .unwrap()
        .contains("conversions"));
    assert_eq!(window.current.spend, 70.0);
}

#[test]
fn a_weekday_pattern_does_not_trigger_a_movement_or_alert() {
    let rows = (0..=62)
        .map(|day| CampaignDay {
            day,
            campaign_id: 701,
            metrics: if day % 7 < 5 {
                metrics(100.0, 50.0, 5.0)
            } else {
                metrics(0.0, 0.0, 0.0)
            },
        })
        .collect::<Vec<_>>();
    assert!(detection(&rows, 0, true).quiet());
    assert_eq!(
        alerts::detect(
            &rows,
            &[campaign()],
            62,
            0,
            AllFollowAccount { judged: true },
            "Dental",
            "USD"
        ),
        vec![]
    );
}

#[test]
fn a_zero_spend_campaign_has_no_movement() {
    let rows = (0..=62)
        .map(|day| CampaignDay {
            day,
            campaign_id: 701,
            metrics: metrics(0.0, 0.0, 0.0),
        })
        .collect::<Vec<_>>();
    assert!(detection(&rows, 0, false).quiet());
}

#[test]
fn a_campaign_movement_exceeds_its_own_eight_week_variation() {
    let mut rows = days(62);
    for row in &mut rows {
        row.metrics.spend = if (row.day / 7) % 2 == 0 { 10.0 } else { 20.0 };
    }
    assert!(detection(&rows, 0, true).movements.is_empty());
    for row in rows.iter_mut().filter(|row| row.day >= 56) {
        row.metrics.spend = 50.0;
    }
    let detected = detection(&rows, 0, true);
    assert_eq!(detected.campaigns[0].campaign_id, 701);
    assert_eq!(
        detected.campaigns[0].movements[0],
        detection::Movement {
            metric: detection::MetricName::Spend,
            current: 350.0,
            previous: 140.0,
            normal_variation: 70.0
        }
    );
    assert_eq!(detection(&rows, 0, true), detected);
}

#[test]
fn a_new_ad_account_uses_the_conservative_weekly_threshold() {
    let mut rows = days(62)
        .into_iter()
        .filter(|row| row.day >= 49)
        .collect::<Vec<_>>();
    for row in rows.iter_mut().filter(|row| row.day >= 56) {
        row.metrics.spend = 14.0;
    }
    assert!(detection(&rows, 49, true).quiet());
    for row in rows.iter_mut().filter(|row| row.day >= 56) {
        row.metrics.spend = 20.0;
    }
    assert_eq!(
        detection(&rows, 49, true).campaigns[0].movements[0].normal_variation,
        35.0
    );
}

#[test]
fn an_incomplete_window_does_not_compare_missing_history_as_zero() {
    let rows = days(62)
        .into_iter()
        .filter(|row| row.day >= 60)
        .collect::<Vec<_>>();
    assert!(detection(&rows, 60, true).movements.is_empty());
}

#[test]
fn a_budget_account_change_triggers_a_campaign_line_on_a_quiet_week() {
    let change = AccountChange {
        changed_at: "1970-03-04 10:00:00".into(),
        resource_name: "change/1".into(),
        resource_type: "CAMPAIGN_BUDGET".into(),
        operation: "UPDATE".into(),
        campaign_id: Some(701),
        changed_fields: "amountMicros".into(),
        old_value: json!({"campaignBudget":{"amountMicros":"80000000"}}).to_string(),
        new_value: json!({"campaignBudget":{"amountMicros":"0"}}).to_string(),
    };
    let detected = detection::detect(
        &days(62),
        &[campaign()],
        &[change],
        62,
        0,
        AllFollowAccount { judged: true },
        "USD",
    );
    assert_eq!(detected.campaigns.len(), 1);
    assert_eq!(detected.campaigns[0].movements, vec![]);
    assert_eq!(
        detected.campaigns[0].reasons,
        vec!["Account change in the last 7 days"]
    );
    assert_eq!(
        detected.yesterday,
        vec!["Search - Real: Daily budget changed from $80.00 to $0.00 on 4 March"]
    );
}

#[test]
fn a_campaign_that_started_yesterday_gets_a_line() {
    let rows = vec![CampaignDay {
        day: 62,
        campaign_id: 701,
        metrics: metrics(1.0, 1.0, 0.0),
    }];
    let detected = detection(&rows, 0, true);
    assert_eq!(
        detected.campaigns[0].reasons,
        vec!["Campaign started in the last 7 days"]
    );
    assert_eq!(detected.yesterday, vec!["Search - Real started yesterday."]);
}

#[test]
fn daily_alerts_use_matching_weekdays_and_do_not_repeat_a_stopped_event() {
    let mut rows = days(62);
    rows[62].metrics = metrics(0.0, 0.0, 0.0);
    let found = alerts::detect(
        &rows,
        &[campaign()],
        62,
        0,
        AllFollowAccount { judged: true },
        "Dental",
        "USD",
    );
    assert_eq!(
        found
            .iter()
            .map(|alert| alert.event_key)
            .collect::<Vec<_>>(),
        vec![alerts::EventKey::CampaignStopped(701)]
    );
    assert_eq!(found[0].day, 62);
    rows.push(CampaignDay {
        day: 63,
        campaign_id: 701,
        metrics: metrics(0.0, 0.0, 0.0),
    });
    assert!(alerts::detect(
        &rows,
        &[campaign()],
        63,
        0,
        AllFollowAccount { judged: true },
        "Dental",
        "USD"
    )
    .is_empty());
}

#[test]
fn doubled_spend_alert_is_inclusive_and_conversion_alerts_need_results() {
    let mut rows = days(62);
    rows[62].metrics = metrics(20.0, 10.0, 0.0);
    let found = alerts::detect(
        &rows,
        &[campaign()],
        62,
        0,
        AllFollowAccount { judged: false },
        "Dental",
        "USD",
    );
    assert_eq!(
        found
            .iter()
            .map(|alert| alert.event_key)
            .collect::<Vec<_>>(),
        vec![alerts::EventKey::SpendDoubled]
    );
    assert_eq!(found[0].normal_value, 10.0);
    rows[62].metrics.spend = 19.99;
    assert!(alerts::detect(
        &rows,
        &[campaign()],
        62,
        0,
        AllFollowAccount { judged: false },
        "Dental",
        "USD"
    )
    .is_empty());
}

#[test]
fn conversion_tracking_distinguishes_setup_and_supports_both_user_choices() {
    assert_eq!(
        tracking::state(100.0, Some(0)),
        ConversionTracking::NotSetUp
    );
    assert_eq!(
        tracking::state(9.99, Some(1)),
        ConversionTracking::TooFewResults
    );
    assert_eq!(tracking::state(10.0, Some(1)), ConversionTracking::Working);
    assert_eq!(
        tracking::state(0.0, None),
        ConversionTracking::TooFewResults
    );
    assert!(tracking::judge(
        ConversionTracking::TooFewResults,
        Some(true)
    ));
    assert!(!tracking::judge(ConversionTracking::Working, Some(false)));
    assert!(!tracking::judge(ConversionTracking::NotSetUp, None));
}

fn moved() -> detection::Detection {
    let mut rows = days(62);
    for row in rows.iter_mut().filter(|row| row.day >= 56) {
        row.metrics.spend = 20.0;
    }
    detection(&rows, 0, true)
}

#[test]
fn an_update_accepts_only_supplied_numbers_and_listed_campaigns() {
    let detected = moved();
    let accepted=update::accept(json!({"summary":"Spend increased.","lines":[{"campaign_id":701,"text":"Search - Real spend increased from 70 to 140."}]}),&detected).unwrap();
    assert_eq!(accepted.summary, "Spend increased.");
    assert_eq!(
        accepted.lines[0].text,
        "Search - Real spend increased from 70 to 140."
    );
    for rejected in [
        json!({"summary":"Spend was 999999.","lines":[{"campaign_id":701,"text":"Spend increased."}]}),
        json!({"summary":"Spend increased.","lines":[{"campaign_id":999,"text":"Spend increased."}]}),
        json!({"summary":"Spend increased in Atlantis.","lines":[{"campaign_id":701,"text":"Spend increased."}]}),
        json!({"summary":"Spend increased because clicks rose.","lines":[{"campaign_id":701,"text":"Spend increased."}]}),
        json!({"summary":"Spend increased.","lines":[],"extra":"anything"}),
        json!({"summary":"x".repeat(321),"lines":[{"campaign_id":701,"text":"Spend increased."}]}),
        json!({"summary":"Spend increased.","lines":[{"campaign_id":701,"text":"x".repeat(221)}]}),
    ] {
        assert!(update::accept(rejected, &detected).is_err());
    }
}

#[test]
fn a_campaign_line_cannot_borrow_another_campaigns_numbers() {
    let mut detected = moved();
    let mut other = detected.campaigns[0].clone();
    other.campaign_id = 702;
    other.name = "Brand".into();
    other.metrics.current.spend = 9999.0;
    detected.campaigns.push(other);
    assert!(update::accept(json!({"summary":"Spend increased.","lines":[{"campaign_id":701,"text":"Spend was 9999."},{"campaign_id":702,"text":"Spend increased."}]}),&detected).is_err());
}

const BUDGET_EVIDENCE: &str = "Daily budget changed from $50.00 to $80.00 on 7 October. Spend averaged $25.00 a day after it, against $10.00 a day in the previous 7 days.";

fn candidate(
    id: &str,
    level: Level,
    campaign_id: Option<i64>,
    evidence: &str,
) -> causes::Candidate {
    causes::Candidate {
        id: id.into(),
        kind: causes::Kind::AccountChange,
        level,
        metric: detection::MetricName::Spend,
        campaign_id,
        evidence: evidence.into(),
        anchors: vec!["7 October".into()],
    }
}

fn with_causes(candidates: Vec<causes::Candidate>) -> detection::Detection {
    let mut detected = moved();
    detected.causes = candidates;
    detected
}

fn line_answer(text: &str, cause: Option<&str>) -> Value {
    json!({"summary":"Spend increased.","cause":null,"lines":[{"campaign_id":701,"text":text,"cause":cause}]})
}

#[test]
fn an_update_may_say_because_for_a_confirmed_cause_and_stores_its_evidence() {
    let detected = with_causes(vec![
        candidate("701/budget", Level::Confirmed, Some(701), BUDGET_EVIDENCE),
        candidate("account/budget", Level::Confirmed, None, BUDGET_EVIDENCE),
    ]);
    let accepted = update::accept(
        json!({"summary":"Spend rose to 140 because the daily budget changed from $50.00 to $80.00 on 7 October.","cause":"account/budget","lines":[{"campaign_id":701,"text":"Spend rose to 140 after a daily budget of $80.00 on 7 October, averaging $25.00 a day.","cause":"701/budget"}]}),
        &detected,
    )
    .unwrap();
    let expected = Some(causes::Cause {
        level: Level::Confirmed,
        evidence: BUDGET_EVIDENCE.into(),
    });
    assert_eq!(accepted.cause, expected);
    assert_eq!(accepted.lines[0].cause, expected);
}

#[test]
fn an_update_must_call_a_likely_cause_likely() {
    let detected = with_causes(vec![candidate(
        "701/budget",
        Level::Likely,
        Some(701),
        BUDGET_EVIDENCE,
    )]);
    assert!(update::accept(
        line_answer(
            "Spend rose to 140 because the daily budget changed on 7 October.",
            Some("701/budget")
        ),
        &detected
    )
    .is_err());
    let accepted = update::accept(
        line_answer(
            "Spend rose to 140, most likely because the daily budget changed on 7 October.",
            Some("701/budget"),
        ),
        &detected,
    )
    .unwrap();
    assert_eq!(
        accepted.lines[0].cause.as_ref().map(|cause| cause.level),
        Some(Level::Likely)
    );
}

#[test]
fn an_update_without_a_cause_cannot_use_causal_words() {
    let detected = with_causes(vec![candidate(
        "701/budget",
        Level::Confirmed,
        Some(701),
        BUDGET_EVIDENCE,
    )]);
    assert!(update::accept(
        line_answer("Spend rose to 140 because the daily budget changed.", None),
        &detected
    )
    .is_err());
}

#[test]
fn an_update_can_state_only_a_cause_that_the_software_listed_for_that_text() {
    let detected = with_causes(vec![
        candidate("701/budget", Level::Confirmed, Some(701), BUDGET_EVIDENCE),
        candidate("account/budget", Level::Confirmed, None, BUDGET_EVIDENCE),
    ]);
    for answer in [
        line_answer("Spend rose to 140.", Some("invented")),
        line_answer("Spend rose to 140.", Some("account/budget")),
        json!({"summary":"Spend rose to 140.","cause":"701/budget","lines":[{"campaign_id":701,"text":"Spend rose to 140.","cause":null}]}),
    ] {
        assert!(update::accept(answer, &detected).is_err());
    }
}

#[test]
fn a_cause_gives_the_update_its_evidence_numbers_and_no_others() {
    let detected = with_causes(vec![candidate(
        "701/budget",
        Level::Confirmed,
        Some(701),
        BUDGET_EVIDENCE,
    )]);
    assert!(update::accept(
        line_answer(
            "Spend averaged $25.00 a day because the daily budget changed on 7 October.",
            Some("701/budget")
        ),
        &detected
    )
    .is_ok());
    assert!(update::accept(
        line_answer(
            "Spend averaged $25.00 a day, because the daily budget changed on 7 October.",
            None
        ),
        &detected
    )
    .is_err());
    assert!(update::accept(
        line_answer(
            "Spend averaged $26.00 a day because the daily budget changed on 7 October.",
            Some("701/budget")
        ),
        &detected
    )
    .is_err());
}

fn contribution() -> causes::Candidate {
    causes::Candidate {
        kind: causes::Kind::Concentration,
        anchors: vec![],
        ..candidate(
            "701/concentration",
            Level::Contribution,
            Some(701),
            "Mobile accounts for 70% of the rise in Spend: $70.00 of $100.00.",
        )
    }
}

#[test]
fn a_contribution_unlocks_no_causal_words_and_is_stored_as_a_contribution() {
    let detected = with_causes(vec![contribution()]);
    assert!(update::accept(
        line_answer(
            "Spend rose to 140 because the daily budget was lowered.",
            Some("701/concentration")
        ),
        &detected
    )
    .is_err());
    let accepted = update::accept(
        line_answer(
            "Spend rose to 140, mostly on Mobile.",
            Some("701/concentration"),
        ),
        &detected,
    )
    .unwrap();
    assert_eq!(
        accepted.lines[0].cause.as_ref().map(|cause| cause.level),
        Some(Level::Contribution)
    );
}

#[test]
fn a_causal_word_must_sit_in_a_sentence_that_names_the_chosen_cause() {
    let detected = with_causes(vec![candidate(
        "701/budget",
        Level::Confirmed,
        Some(701),
        BUDGET_EVIDENCE,
    )]);
    for text in [
        "Spend rose to 140 because the weather changed. The budget changed on 7 October.",
        "Spend rose to 140 because the weather changed.",
    ] {
        assert!(
            update::accept(line_answer(text, Some("701/budget")), &detected).is_err(),
            "{text}"
        );
    }
    assert!(update::accept(
        line_answer(
            "Spend rose to 140 because the daily budget changed on 7 October.",
            Some("701/budget")
        ),
        &detected
    )
    .is_ok());
}

#[test]
fn the_likelihood_word_of_a_likely_cause_must_sit_in_the_sentence_of_the_causal_word() {
    let detected = with_causes(vec![candidate(
        "701/budget",
        Level::Likely,
        Some(701),
        BUDGET_EVIDENCE,
    )]);
    assert!(update::accept(
        line_answer(
            "Spend rose to 140 because the daily budget changed on 7 October. Most likely it will hold.",
            Some("701/budget")
        ),
        &detected
    )
    .is_err());
}

#[test]
fn a_campaign_name_with_a_likelihood_word_does_not_make_a_cause_likely() {
    let mut detected = with_causes(vec![candidate(
        "701/budget",
        Level::Likely,
        Some(701),
        BUDGET_EVIDENCE,
    )]);
    detected.campaigns[0].name = "Probably Shoes".into();
    assert!(update::accept(
        line_answer(
            "Spend rose to 140 because the daily budget changed on 7 October in Probably Shoes.",
            Some("701/budget")
        ),
        &detected
    )
    .is_err());
}

#[test]
fn without_ai_each_text_states_its_first_cause_in_code_written_words_with_its_level() {
    let detected = with_causes(vec![
        candidate("701/a", Level::Likely, Some(701), "Bid strategy changed."),
        candidate("701/b", Level::Confirmed, Some(701), BUDGET_EVIDENCE),
        candidate("account/a", Level::Confirmed, None, BUDGET_EVIDENCE),
    ]);
    let stated = update::fallback(&detected);
    assert_eq!(
        stated.lines[0].text,
        "Spend was $140 against $70.00 in the previous 7 days. Spend rose, most likely because of the Account change on 7 October."
    );
    assert_eq!(
        stated.lines[0].cause,
        Some(causes::Cause {
            level: Level::Likely,
            evidence: "Bid strategy changed.".into()
        })
    );
    assert_eq!(
        stated.summary,
        "Spend was $140 against $70.00 in the previous 7 days. Spend rose because of the Account change on 7 October. Yesterday spend was $20.00, against a usual $10.00 for that weekday."
    );
    assert_eq!(
        stated.cause,
        Some(causes::Cause {
            level: Level::Confirmed,
            evidence: BUDGET_EVIDENCE.into()
        })
    );
    assert_eq!(update::fallback(&moved()).cause, None);
}

#[test]
fn without_ai_a_contribution_is_shown_without_a_reason_in_the_text() {
    let detected = with_causes(vec![contribution()]);
    let stated = update::fallback(&detected);
    assert_eq!(
        stated.lines[0].text,
        "Spend was $140 against $70.00 in the previous 7 days."
    );
    assert_eq!(
        stated.lines[0].cause.as_ref().map(|cause| cause.level),
        Some(Level::Contribution)
    );
}

#[test]
fn an_update_stored_before_causes_existed_is_still_readable() {
    let mut stored = serde_json::to_value(update::fallback(&moved())).unwrap();
    stored["detection"]
        .as_object_mut()
        .unwrap()
        .remove("causes");
    let read: update::Update = serde_json::from_value(stored).unwrap();
    assert_eq!(read.cause, None);
    assert_eq!(read.lines[0].cause, None);
    assert!(read.detection.causes.is_empty());
}

#[test]
fn deep_check_campaign_findings_carry_ids_even_when_names_match() {
    let data = Data {
        currency: "USD".into(),
        reporting: Counts {
            cost: 1000.0,
            clicks: 100.0,
            conversions: 10.0,
            ..Default::default()
        },
        audit: Audit {
            conversion_actions: Some(1),
            ..Default::default()
        },
    };
    let audit = Audit {
        campaigns: vec![AuditRow {
            name: "Shared".into(),
            counts: Counts {
                cost: 600.0,
                clicks: 40.0,
                conversions: 1.0,
                ..Default::default()
            },
            ..Default::default()
        }],
        ..Default::default()
    };
    let by_campaign = BTreeMap::from([(701, audit.clone()), (702, audit)]);
    let findings = deep_check::detect(&data, &by_campaign, AllFollowAccount { judged: true });
    assert_eq!(findings[0].rule.detail,"Shared spent $600 for 1 conversions ($600 per conversion). The Ad Account cost per conversion is $100. It had 60% of the spend and 10% of the conversions.");
    assert_eq!(
        findings
            .iter()
            .map(|finding| (finding.rule.key.as_str(), finding.campaign_id))
            .collect::<Vec<_>>(),
        vec![
            ("campaign-cost-high:701:Shared (701)", Some(701)),
            ("campaign-cost-high:702:Shared (702)", Some(702))
        ]
    );
    assert!(
        deep_check::detect(&data, &by_campaign, AllFollowAccount { judged: false })
            .iter()
            .all(|finding| !finding.rule.key.starts_with("campaign-cost-high"))
    );
}

#[test]
fn trial_and_subscription_access_expire_at_the_exact_boundary() {
    assert!(access::trial_open(100, 100 + 14 * 86_400 - 1));
    assert!(!access::trial_open(100, 100 + 14 * 86_400));
    assert!(!access::trial_open(100, 99));
    let mut subscription = Subscription {
        id: "sub".into(),
        created_at: 0,
        status: crate::billing::SubscriptionStatus::Active,
        period_start: 0,
        period_end: 1000,
    };
    assert!(access::subscription_open(Some(&subscription), 999));
    assert!(!access::subscription_open(Some(&subscription), 1000));
    subscription.status = crate::billing::SubscriptionStatus::Canceled;
    assert!(!access::subscription_open(Some(&subscription), 999));
}

#[test]
fn rounded_currency_and_normal_sentence_openers_are_accepted() {
    let mut detected = moved();
    detected.metrics.current.spend = 99.28999999999999;
    detected.campaigns[0].metrics.current.spend = 99.28999999999999;
    for summary in [
        "In the last 7 days, spend was $99.29.",
        "Overall, spend was $99.29.",
        "Add negative keywords.",
        "Consider reviewing spend.",
        "During this window, spend was $99.29.",
    ] {
        assert!(update::accept(json!({"summary":summary,"lines":[{"campaign_id":701,"text":"In the last 7 days, Search - Real spent $99.29."}]}), &detected).is_ok(), "{summary}");
    }
}

#[test]
fn update_known_numbers_exclude_ids_and_variation_and_reject_causes() {
    let detected = moved();
    for summary in [
        "Spend was 701.",
        "Spend was 63.",
        "Spend was 1.",
        "Spend was driven by clicks.",
        "Spend led to clicks.",
    ] {
        assert!(
            update::accept(
                json!({"summary":summary,"lines":[{"campaign_id":701,"text":"Spend increased."}]}),
                &detected
            )
            .is_err(),
            "{summary}"
        );
    }
    assert!(update::accept(json!({"summary":"Spend increased after clicks changed.","lines":[{"campaign_id":701,"text":"Spend increased."}]}), &detected).is_ok());
}

#[test]
fn spend_alert_baseline_starts_when_data_starts_and_has_a_floor() {
    let mut rows = days(62);
    for row in &mut rows {
        row.metrics = metrics(0.0, 0.0, 0.0);
    }
    for row in rows.iter_mut().filter(|row| row.day >= 35) {
        row.metrics = metrics(10.0, 10.0, 1.0);
    }
    assert!(alerts::detect(
        &rows,
        &[],
        62,
        0,
        AllFollowAccount { judged: false },
        "Dental",
        "USD"
    )
    .is_empty());
    for row in &mut rows {
        row.metrics = metrics(0.1, 1.0, 0.0);
    }
    rows[62].metrics.spend = 0.2;
    assert!(alerts::detect(
        &rows,
        &[],
        62,
        0,
        AllFollowAccount { judged: false },
        "Dental",
        "USD"
    )
    .is_empty());
}

#[test]
fn zero_conversions_need_three_normally_and_two_zero_days() {
    let mut rows = days(62);
    for row in &mut rows {
        row.metrics.conversions = Some(3.0);
    }
    rows[62].metrics.conversions = Some(0.0);
    assert!(alerts::detect(
        &rows,
        &[],
        62,
        0,
        AllFollowAccount { judged: true },
        "Dental",
        "USD"
    )
    .is_empty());
    rows[61].metrics.conversions = Some(0.0);
    assert!(
        alerts::detect(
            &rows,
            &[],
            62,
            0,
            AllFollowAccount { judged: true },
            "Dental",
            "USD"
        )
        .is_empty(),
        "conversions can still arrive for the newest two days"
    );
    rows[60].metrics.conversions = Some(0.0);
    let found = alerts::detect(
        &rows,
        &[],
        62,
        0,
        AllFollowAccount { judged: true },
        "Dental",
        "USD",
    );
    assert_eq!(found[0].event_key, alerts::EventKey::ConversionsZero);
    assert_eq!(found[0].day, 61);
}

#[test]
fn a_conversion_movement_ignores_yesterday_and_judges_older_days() {
    let mut rows = days(62);
    for row in &mut rows {
        row.metrics.conversions = Some(10.0);
    }
    rows[62].metrics.conversions = Some(0.0);
    let detected = detection(&rows, 0, true);
    assert!(detected
        .movements
        .iter()
        .all(|movement| movement.metric != detection::MetricName::Conversions));
    assert!(detected
        .yesterday
        .iter()
        .all(|fact| !fact.to_lowercase().contains("conversions")));
    for row in rows.iter_mut().filter(|row| (55..=61).contains(&row.day)) {
        row.metrics.conversions = Some(20.0);
    }
    let detected = detection(&rows, 0, true);
    assert_eq!(
        detected
            .movements
            .iter()
            .find(|movement| movement.metric == detection::MetricName::Conversions)
            .map(|movement| (movement.current, movement.previous)),
        Some((140.0, 70.0))
    );
    assert_eq!(detected.metrics.current.conversions, Some(140.0));
    assert!(detected
        .yesterday
        .iter()
        .any(|fact| fact.contains("conversions were 20, against a usual 10")));
}

#[test]
fn finding_keys_keep_each_subject_in_one_campaign() {
    assert_eq!(
        deep_check::finding_key("keyword-cost-high:alpha", 701),
        "keyword-cost-high:701:alpha"
    );
    assert_eq!(
        deep_check::finding_key("keyword-cost-high:beta", 701),
        "keyword-cost-high:701:beta"
    );
    assert_eq!(
        deep_check::finding_key("keyword-cost-high:a:b", 701),
        "keyword-cost-high:701:a:b"
    );
}

#[test]
fn the_key_of_a_finding_about_the_campaign_itself_ends_with_the_campaign_id() {
    assert_eq!(
        deep_check::finding_key("budget-limited:Search - Brand", 701),
        "budget-limited:701:Search - Brand (701)"
    );
    assert_eq!(
        deep_check::finding_key("tracking-missing", 701),
        "tracking-missing:701"
    );
}

#[test]
fn update_limits_campaign_lines_to_five_largest_movements() {
    let mut detected = moved();
    let original = detected.campaigns[0].clone();
    detected.campaigns.clear();
    for index in 0..8 {
        let mut campaign = original.clone();
        campaign.campaign_id = 701 + index;
        campaign.name = format!("Campaign {index}");
        campaign.metrics.current.spend = 100.0 * (index + 1) as f64;
        detected.campaigns.push(campaign);
    }
    let facts = update::facts(&detected);
    assert_eq!(facts["campaigns"].as_array().unwrap().len(), 5);
    assert_eq!(facts["campaigns"][0]["campaign_id"], 708);
    assert_eq!(facts["campaigns"][4]["campaign_id"], 704);
    assert_eq!(facts["more_campaigns"], "3");
    let answer = json!({"summary":"Spend changed.","lines":facts["campaigns"].as_array().unwrap().iter().map(|campaign|json!({"campaign_id":campaign["campaign_id"],"text":"Spend changed."})).collect::<Vec<_>>()});
    let accepted = update::accept(answer, &detected).unwrap();
    assert_eq!(accepted.more_campaigns, 3);
    assert_eq!(accepted.lines.len(), 5);
    let fallback = update::fallback(&detected);
    assert_eq!(fallback.more_campaigns, 3);
    assert_eq!(fallback.lines[0].campaign_id, 708);
    assert!(fallback.written_without_ai);
    for campaign in &mut detected.campaigns {
        campaign.metrics.current.spend = 70.0;
        campaign.metrics.current.clicks = campaign.campaign_id as f64;
    }
    assert_eq!(update::facts(&detected)["campaigns"][0]["campaign_id"], 708);
}

#[test]
fn formatted_facts_use_currency_counts_and_percent_direction() {
    assert_eq!(metrics::english_money(99.28999999999999, "USD"), "$99.29");
    assert_eq!(metrics::english_money(1234.0, "EUR"), "EUR 1,234");
    assert_eq!(metrics::english_money(1234567.0, "RON"), "RON 1,234,567");
    assert_eq!(metrics::english_money(59.5, "RON"), "RON 59.50");
    assert_eq!(metrics::english_count(12345.0), "12,345");
    assert_eq!(metrics::english_change(99.29, 100.0), "down 0.7%");
    assert_eq!(metrics::english_change(150.0, 100.0), "up 50%");
    assert_eq!(metrics::english_change(118.2, 100.0), "up 18%");
    assert_eq!(metrics::english_change(110.0, 100.0), "up 10%");
    assert_eq!(metrics::english_change(109.96, 100.0), "up 10%");
    assert_eq!(metrics::english_change(109.9, 100.0), "up 9.9%");
    assert_eq!(metrics::english_change(90.0, 100.0), "down 10%");
    assert_eq!(metrics::english_change(90.1, 100.0), "down 9.9%");
    assert_eq!(metrics::english_change(100.04, 100.0), "no change");
}

#[test]
fn recent_budget_increase_suppresses_spend_alert_only() {
    let change = AccountChange {
        changed_at: "1970-03-04 10:00:00".into(),
        resource_name: "budget/1".into(),
        resource_type: "CAMPAIGN_BUDGET".into(),
        operation: "UPDATE".into(),
        campaign_id: Some(701),
        changed_fields: "amountMicros".into(),
        old_value: json!({"campaignBudget":{"amountMicros":"10000000"}}).to_string(),
        new_value: json!({"campaignBudget":{"amountMicros":"20000000"}}).to_string(),
    };
    assert!(alerts::budget_raised(std::slice::from_ref(&change), 62));
    assert!(!alerts::budget_raised(std::slice::from_ref(&change), 69));
    let mut reduced = change;
    std::mem::swap(&mut reduced.old_value, &mut reduced.new_value);
    assert!(!alerts::budget_raised(&[reduced], 62));
}

#[test]
fn updates_require_literal_money_counts_and_percent_strings() {
    let mut detected = moved();
    detected.metrics.current.spend = 99.28999999999999;
    detected.metrics.current.clicks = 12345.0;
    detected.campaigns[0].metrics = detected.metrics.clone();
    let line = json!({"campaign_id":701,"text":"Overall, Search - Real spent $99.29."});
    assert!(update::accept(json!({"summary":"Overall, spend was $99.29 with 12,345 clicks, a 42% increase in spend.","lines":[line.clone()]}),&detected).is_ok());
    for summary in [
        "Spend was $99.290.",
        "Spend was EUR99.29.",
        "Clicks were 12345.",
        "Spend rose 41.80%.",
    ] {
        assert!(
            update::accept(json!({"summary":summary,"lines":[line.clone()]}), &detected).is_err(),
            "{summary}"
        );
    }
    let input = update::facts(&detected).to_string();
    assert!(!input.contains("normal_variation"));
    assert!(!input.contains("end_day"));
    assert!(!input.contains("search_terms"));
    assert!(!input.contains("99.28999999999999"));
}

#[test]
fn findings_for_two_subjects_in_one_campaign_keep_distinct_keys() {
    let data = Data {
        currency: "USD".into(),
        reporting: Counts {
            cost: 1000.0,
            clicks: 1000.0,
            conversions: 100.0,
            ..Default::default()
        },
        audit: Audit {
            conversion_actions: Some(1),
            ..Default::default()
        },
    };
    let audit = Audit {
        landing_pages: ["https://example.com/a", "https://example.com/b"]
            .into_iter()
            .map(|name| AuditRow {
                name: name.into(),
                counts: Counts {
                    cost: 200.0,
                    clicks: 100.0,
                    conversions: 1.0,
                    ..Default::default()
                },
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    let findings = deep_check::detect(
        &data,
        &BTreeMap::from([(701, audit)]),
        AllFollowAccount { judged: true },
    );
    assert_eq!(
        findings
            .iter()
            .map(|finding| finding.rule.key.as_str())
            .collect::<Vec<_>>(),
        vec![
            "landing-page-low-rate:701:https://example.com/a",
            "landing-page-low-rate:701:https://example.com/b"
        ]
    );
}

#[test]
fn stopped_serving_is_per_campaign_and_conversion_alerts_are_per_account() {
    let mut rows = days(62);
    for row in &mut rows {
        row.metrics.conversions = Some(3.0);
    }
    let mut other = campaign();
    other.id = 702;
    other.name = "Brand".into();
    rows.extend(days(62).into_iter().map(|mut row| {
        row.campaign_id = 702;
        row.metrics.conversions = Some(3.0);
        row
    }));
    rows.iter_mut()
        .filter(|row| row.campaign_id == 701 && row.day >= 61)
        .for_each(|row| row.metrics = metrics(0.0, 0.0, 0.0));
    let found = alerts::detect(
        &rows,
        &[campaign(), other],
        61,
        0,
        AllFollowAccount { judged: true },
        "Dental",
        "USD",
    );
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].event_key, alerts::EventKey::CampaignStopped(701));
    assert_eq!(found[0].campaign_id, Some(701));
    assert!(alerts::detect(
        &rows,
        &[],
        62,
        0,
        AllFollowAccount { judged: true },
        "Dental",
        "USD"
    )
    .is_empty());
    rows.iter_mut()
        .filter(|row| row.day >= 60)
        .for_each(|row| row.metrics.conversions = Some(0.0));
    let found = alerts::detect(
        &rows,
        &[],
        62,
        0,
        AllFollowAccount { judged: true },
        "Dental",
        "USD",
    );
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].event_key, alerts::EventKey::ConversionsZero);
    assert_eq!(found[0].campaign_id, None);
}

#[test]
fn review_b3_fallback_states_account_movements_and_yesterday() {
    let mut detected = moved();
    detected.campaigns.clear();
    detected.yesterday =
        vec!["Yesterday spend was $20.00, against a usual $10.00 for that weekday.".into()];
    let stored = update::fallback(&detected);
    assert!(stored.lines.is_empty());
    assert!(stored
        .summary
        .contains("Spend was $140 against $70.00 in the previous 7 days."));
    assert!(stored.summary.contains(&detected.yesterday[0]));
    detected.movements.clear();
    assert!(update::fallback(&detected)
        .summary
        .contains(&detected.yesterday[0]));
}

#[test]
fn review_b4_fallback_bounds_lines_and_keeps_two_newest_changes() {
    let mut detected = moved();
    detected.campaigns[0].account_changes = vec![
        "2026-10-11 Budget EUR20.00".into(),
        "2026-10-10 Budget EUR30.00".into(),
        "2026-10-09 Budget EUR40.00".into(),
        "2026-10-08 Budget EUR45.00".into(),
    ];
    let line = &update::fallback(&detected).lines[0].text;
    assert!(line.contains("2026-10-11 Budget EUR20.00"));
    assert!(line.contains("2026-10-10 Budget EUR30.00"));
    assert!(!line.contains("2026-10-09"));
    assert!(line.ends_with("and 2 more changes."));
    detected.campaigns[0].name = "Long Campaign 名".repeat(30);
    assert!(update::fallback(&detected).lines[0].text.chars().count() <= 220);
    assert!(update::fallback(&detected).lines[0]
        .text
        .ends_with("and 2 more changes."));
}

#[test]
fn review_b5_rounded_zero_is_no_change() {
    assert_eq!(metrics::english_change(100.001, 100.0), "no change");
    assert_eq!(metrics::english_change(99.999, 100.0), "no change");
    assert_eq!(metrics::english_change(100.0, 100.0), "no change");
}

#[test]
fn fallback_orders_account_changes_by_newest_timestamp() {
    let changes = [1, 4, 2, 3].map(|day| AccountChange {
        changed_at: format!("1970-03-{day:02} 10:00:00"),
        resource_name: format!("budget/{day}"),
        resource_type: "CAMPAIGN_BUDGET".into(),
        operation: "UPDATE".into(),
        campaign_id: Some(701),
        changed_fields: "amountMicros".into(),
        old_value: json!({"campaignBudget":{"amountMicros":"45000000"}}).to_string(),
        new_value: json!({"campaignBudget":{"amountMicros":"20000000"}}).to_string(),
    });
    let detected = detection::detect(
        &days(62),
        &[campaign()],
        &changes,
        62,
        0,
        AllFollowAccount { judged: true },
        "EUR",
    );
    let line = &update::fallback(&detected).lines[0].text;
    assert!(line.find("on 4 March").unwrap() < line.find("on 3 March").unwrap());
    assert!(!line.contains("on 2 March"));
    assert!(!line.contains("on 1 March"));
    assert!(line.ends_with("and 2 more changes."));
    assert!(line.chars().count() <= 220);
}

#[test]
fn a_campaign_name_that_a_mail_client_would_link_is_neutralised_and_capped_in_the_alert_subject() {
    let stopped = alerts::Alert {
        day: 20_000,
        event_key: alerts::EventKey::CampaignStopped(701),
        campaign_id: Some(701),
        name: "Visit shop.example.com or me@example.com https://example.org".into(),
        metrics: metrics(0.0, 0.0, 0.0),
        normal_value: 0.0,
        body: String::new(),
    };
    let message = alerts::message(
        Language::En,
        &stopped,
        "4829153307",
        "Dental",
        "USD",
        "https://keysend.app",
    )
    .unwrap();
    assert_eq!(
        message.subject,
        "Alert: Visit shop[.]example[.]com or me[at]example[.]com https[:]//example[.]org stopped in Dental"
    );
    assert!(!message.html.contains("shop.example.com"));
    assert!(!message.text.contains("example.org"));
    assert!(message.html.contains(r#"<html lang="en">"#));
    assert!(message.html.contains(">Open Dental</a>"));
    let long = alerts::Alert {
        name: "A".repeat(200),
        ..stopped
    };
    let message = alerts::message(
        Language::En,
        &long,
        "4829153307",
        &"B".repeat(200),
        "USD",
        "https://keysend.app",
    )
    .unwrap();
    assert_eq!(
        message.subject,
        format!("Alert: {}… stopped in {}…", "A".repeat(60), "B".repeat(60))
    );
}

#[test]
fn an_ad_is_named_by_its_name_or_its_headlines_or_its_type_and_ad_group() {
    let label = |ad: Value| {
        deep_check::ad_label(&json!({"adGroupAd": {"ad": ad}, "adGroup": {"name": "Emergency"}}))
    };
    assert_eq!(
        label(
            json!({"id": "1", "type": "RESPONSIVE_SEARCH_AD", "responsiveSearchAd": {"headlines": [
                {"text": "Emergency Dentist", "pinnedField": "HEADLINE_1"},
                {"text": "Open Today"},
                {"text": "Harbour Dental"},
                {"text": "Book Online"},
            ]}})
        ),
        "Emergency Dentist | Open Today | Harbour Dental"
    );
    assert_eq!(
        label(
            json!({"id": "2", "type": "EXPANDED_TEXT_AD", "expandedTextAd": {
                "headlinePart1": "Same-Day Appointments",
                "headlinePart2": "Call Harbour Dental",
            }})
        ),
        "Same-Day Appointments | Call Harbour Dental"
    );
    assert_eq!(
        label(json!({"id": "3", "name": "Autumn banner", "type": "IMAGE_AD"})),
        "Autumn banner"
    );
    assert_eq!(
        label(json!({"id": "4", "name": "", "type": "RESPONSIVE_DISPLAY_AD"})),
        "Responsive display ad in ad group Emergency"
    );
    assert_eq!(
        label(json!({"id": "5", "type": "UNKNOWN"})),
        "Ad in ad group Emergency"
    );
    assert_eq!(
        deep_check::ad_label(&json!({"adGroupAd": {"ad": {"id": "6"}}})),
        "Ad without a name"
    );
}

fn rules(account_judged: bool, own: &[(i64, CampaignSettings)]) -> Resolved {
    Resolved::new(
        AccountDefaults {
            state: ConversionTracking::Working,
            judged: account_judged,
            target_cost: Some(45.0),
            alerts: true,
        },
        own.iter().cloned().collect(),
    )
}

fn not_judged() -> CampaignSettings {
    CampaignSettings {
        judge_by_conversions: Choice::No,
        ..Default::default()
    }
}

fn two_campaigns() -> (Vec<CampaignDay>, Vec<Campaign>) {
    let mut other = campaign();
    other.id = 702;
    other.name = "Search - Brand".into();
    let mut rows = Vec::new();
    for id in [701, 702] {
        for day in 0..=62 {
            let recent = day >= 56;
            rows.push(CampaignDay {
                day,
                campaign_id: id,
                metrics: metrics(
                    if recent { 20.0 } else { 10.0 },
                    10.0,
                    if recent { 2.0 } else { 1.0 },
                ),
            });
        }
    }
    (rows, vec![campaign(), other])
}

#[test]
fn a_campaign_that_is_not_judged_has_no_conversions_while_the_ad_account_is_judged() {
    let (rows, campaigns) = two_campaigns();
    let rules = rules(true, &[(702, not_judged())]);
    let detected = detection::detect(&rows, &campaigns, &[], 62, 0, &rules, "USD");
    assert!(detected.judge_by_conversions);
    assert!(detected.metrics.current.conversions.is_some());
    let judged = detected
        .campaigns
        .iter()
        .find(|c| c.campaign_id == 701)
        .unwrap();
    let unjudged = detected
        .campaigns
        .iter()
        .find(|c| c.campaign_id == 702)
        .unwrap();
    assert!(judged.metrics.current.conversions.is_some());
    assert!(judged
        .movements
        .iter()
        .any(|movement| movement.metric == detection::MetricName::Conversions));
    assert_eq!(unjudged.metrics.current.conversions, None);
    assert_eq!(unjudged.metrics.previous.conversion_value, None);
    assert!(unjudged
        .movements
        .iter()
        .all(|movement| movement.metric != detection::MetricName::Conversions));
    let facts = update::facts(&detected);
    let fact = |id: i64| {
        facts["campaigns"]
            .as_array()
            .unwrap()
            .iter()
            .find(|campaign| campaign["campaign_id"] == id)
            .unwrap()
            .clone()
    };
    assert_eq!(fact(701)["judge_by_conversions"], true);
    assert_eq!(fact(702)["judge_by_conversions"], false);
    assert!(!fact(702)
        .to_string()
        .to_lowercase()
        .contains("\"conversions\":"));

    let line = |id: i64, text: &str| json!({"campaign_id":id,"text":text});
    let answer = |text: &str| json!({"summary":"Spend increased.","lines":[line(701, "Conversions rose."),line(702, text)]});
    assert!(update::accept(answer("Spend increased."), &detected).is_ok());
    assert!(update::accept(answer("Conversions rose."), &detected).is_err());
    let fallback = update::fallback(&detected);
    for line in fallback.lines.iter().filter(|line| line.campaign_id == 702) {
        assert!(
            !campaign_settings::mentions_conversions(&line.text),
            "{}",
            line.text
        );
    }
}

#[test]
fn a_campaign_that_chose_yes_is_judged_when_the_ad_account_is_not() {
    let (rows, campaigns) = two_campaigns();
    let yes = CampaignSettings {
        judge_by_conversions: Choice::Yes,
        ..Default::default()
    };
    let detected = detection::detect(
        &rows,
        &campaigns,
        &[],
        62,
        0,
        rules(false, &[(702, yes)]),
        "USD",
    );
    assert!(!detected.judge_by_conversions);
    assert_eq!(detected.metrics.current.conversions, None);
    let judged = detected
        .campaigns
        .iter()
        .find(|c| c.campaign_id == 702)
        .unwrap();
    assert!(judged.metrics.current.conversions.is_some());
    let other = detected
        .campaigns
        .iter()
        .find(|c| c.campaign_id == 701)
        .unwrap();
    assert_eq!(other.metrics.current.conversions, None);
}

#[test]
fn the_findings_of_a_campaign_that_is_not_judged_rest_on_traffic_only() {
    let data = Data {
        currency: "USD".into(),
        reporting: Counts {
            cost: 1000.0,
            clicks: 100.0,
            conversions: 10.0,
            ..Default::default()
        },
        audit: Audit {
            conversion_actions: Some(1),
            ..Default::default()
        },
    };
    let audit = Audit {
        campaigns: vec![AuditRow {
            name: "Shared".into(),
            counts: Counts {
                cost: 600.0,
                clicks: 40.0,
                conversions: 1.0,
                ..Default::default()
            },
            ..Default::default()
        }],
        ads: vec![AuditRow {
            name: "Same-day appointments".into(),
            strength: "POOR".into(),
            counts: Counts {
                impressions: 900.0,
                cost: 300.0,
                clicks: 90.0,
                conversions: 0.0,
                ..Default::default()
            },
            ..Default::default()
        }],
        ..Default::default()
    };
    let by_campaign = BTreeMap::from([(701, audit.clone()), (702, audit)]);
    let findings = deep_check::detect(&data, &by_campaign, rules(true, &[(702, not_judged())]));
    assert!(findings
        .iter()
        .any(|finding| finding.campaign_id == Some(701)
            && campaign_settings::mentions_conversions(&finding.rule.detail)));
    assert!(findings
        .iter()
        .any(|finding| finding.campaign_id == Some(702)));
    for finding in findings
        .iter()
        .filter(|finding| finding.campaign_id == Some(702))
    {
        assert!(
            !campaign_settings::mentions_conversions(&finding.rule.detail)
                && !campaign_settings::mentions_conversions(&finding.rule.title),
            "{finding:?}"
        );
    }
}

#[test]
fn the_stop_alert_of_a_campaign_that_is_not_judged_has_no_conversions() {
    let (mut rows, campaigns) = two_campaigns();
    for row in rows.iter_mut().filter(|row| row.day == 62) {
        row.metrics = metrics(0.0, 0.0, 0.0);
    }
    let found = alerts::detect(
        &rows,
        &campaigns,
        62,
        0,
        rules(true, &[(702, not_judged())]),
        "Dental",
        "USD",
    );
    let stop = |id: i64| {
        found
            .iter()
            .find(|alert| alert.campaign_id == Some(id))
            .unwrap()
    };
    assert!(stop(701).metrics.conversions.is_some());
    assert_eq!(stop(702).metrics.conversions, None);
}

fn weekly_with_campaign_names() -> super::email::WeeklyEmail {
    let rows = days(62);
    let window = metrics::window_metrics(&rows, 62, Window::SevenDays, None, true);
    let update = |summary: &str, lines: &[&str]| {
        serde_json::from_value(json!({
            "day": 62,
            "summary": summary,
            "lines": lines.iter().map(|text| json!({"campaign_id": 701, "text": text})).collect::<Vec<_>>(),
            "more_campaigns": 0,
            "metrics": window,
            "not_useful": false,
            "detection": detection(&rows, 0, true),
        }))
        .unwrap()
    };
    let part = |name: &str, spend, results| super::email::CampaignPart {
        name: name.into(),
        spend,
        results,
    };
    super::email::content(
        62,
        vec![super::email::block(
            "4829153307".into(),
            "Harbour Dental".into(),
            "USD".into(),
            window.clone(),
            vec![
                update(
                    "Search - Emergency spent $120 and Tom & <b>Jerry</b> spent $40.",
                    &["Tom & <b>Jerry</b> lost 20% of its impressions."],
                ),
                update("Display - Remarketing stopped on Monday.", &[]),
            ],
            Some(crate::workspace::monitoring::Recommendation {
                finding_key: "wasted-term:3:jobs".into(),
                action: "Pause Search - Emergency on Sunday.".into(),
                reason: "Display - Remarketing has the same clicks.".into(),
                evidence: "Tom & <b>Jerry</b>: $186 spend.".into(),
                measure: String::new(),
                intent: String::new(),
                subject: String::new(),
                confidence: Default::default(),
                campaign_id: Some(3),
                state: crate::workspace::monitoring::RecommendationState::Open,
            }),
            vec![
                part("Search - Emergency", 120.0, 6.0),
                part("Tom & <b>Jerry</b>", 40.0, 1.0),
                part("Display - Remarketing", 10.0, 1.0),
            ],
        )],
        Vec::new(),
    )
    .unwrap()
}

fn chip(name: &str) -> String {
    format!(
        r#"<b style="padding:1px 5px; border-radius:4px; background:#EEE9F6; color:#3F1D85; font-weight:700;">{name}</b>"#
    )
}

#[test]
fn the_weekly_email_highlights_each_campaign_name_and_not_the_ad_account() {
    let message = super::email::message(
        Language::En,
        &weekly_with_campaign_names(),
        "https://keysend.app",
        &crate::translation::Translations::english(),
    )
    .unwrap();
    let html = message.html;
    let jerry = chip("Tom &amp; &lt;b&gt;Jerry&lt;/b&gt;");
    for expected in [
        format!(
            r#"font-size:19px; line-height:28px;">{} spent $120 and {jerry} spent $40.</td>"#,
            chip("Search - Emergency")
        ),
        format!(r#"<td style="padding:0 0 24px;">{jerry} lost 20% of its impressions.</td>"#),
        format!(
            r#"color:#5C5470;">{jerry} takes 24% of spend and brings 12% of conversions.</td>"#
        ),
        format!(
            r#"font-weight:700;">{}</td>"#,
            chip("Display - Remarketing")
        ),
        format!(
            r#"<br>{} stopped on Monday.</td>"#,
            chip("Display - Remarketing")
        ),
        format!(
            r#"<td style="padding:0 0 4px;">Pause {} on Sunday.</td>"#,
            chip("Search - Emergency")
        ),
        format!(
            r#"color:#5C5470;">{} has the same clicks.</td>"#,
            chip("Display - Remarketing")
        ),
        format!(r#"Finding: {jerry}: $186 spend.</td>"#),
    ] {
        assert!(html.contains(&expected), "{expected}\n{html}");
    }
    assert!(html.contains(">Harbour Dental</h2>"));
    assert!(!html.contains(&chip("Harbour Dental")));
    assert_eq!(html.matches("background:#EEE9F6").count(), 11);
    assert!(!message.text.contains("EEE9F6"));
    assert!(message
        .text
        .contains("Tom & <b>Jerry</b> lost 20% of its impressions."));
}

#[test]
fn a_reader_with_an_english_original_gets_the_chip_inside_the_english_mark() {
    let message = super::email::message(
        Language::Ro,
        &weekly_with_campaign_names(),
        "https://keysend.app",
        &crate::translation::Translations::empty(Language::Ro),
    )
    .unwrap();
    assert!(message.html.contains(&format!(
        r#"<span lang="en">{} spent $120 and {} spent $40.</span>"#,
        chip("Search - Emergency"),
        chip("Tom &amp; &lt;b&gt;Jerry&lt;/b&gt;")
    )));
}

fn stopped_alert(account: &str) -> String {
    let alert = alerts::Alert {
        day: 20_000,
        event_key: alerts::EventKey::CampaignStopped(701),
        campaign_id: Some(701),
        name: "Tom & <b>Jerry</b>".into(),
        metrics: metrics(0.0, 0.0, 0.0),
        normal_value: 0.0,
        body: String::new(),
    };
    alerts::message(
        Language::En,
        &alert,
        "4829153307",
        account,
        "USD",
        "https://keysend.app",
    )
    .unwrap()
    .html
}

#[test]
fn the_stopped_alert_highlights_the_campaign_in_its_headline_and_its_lead_and_not_the_ad_account() {
    let html = stopped_alert("Harbour Dental");
    let jerry = chip("Tom &amp; &lt;b&gt;Jerry&lt;/b&gt;");
    assert!(
        html.contains(&format!("color:#292039;\">{jerry} stopped</h1>")),
        "{html}"
    );
    assert!(
        html.contains(&format!(
            "line-height:28px;\">{jerry} had no impressions, clicks or spend on"
        )),
        "{html}"
    );
    assert!(html.contains("Alert for Harbour Dental, "));
    assert!(html.contains(">Open Harbour Dental</a>"));
    assert_eq!(html.matches("background:#EEE9F6").count(), 2);
}

#[test]
fn the_alerts_that_name_no_campaign_have_no_highlight() {
    for event_key in [
        alerts::EventKey::SpendDoubled,
        alerts::EventKey::ConversionsZero,
    ] {
        let alert = alerts::Alert {
            day: 20_000,
            event_key,
            campaign_id: None,
            name: "Harbour Dental".into(),
            metrics: metrics(300.0, 12.0, 0.0),
            normal_value: 120.0,
            body: String::new(),
        };
        let html = alerts::message(
            Language::En,
            &alert,
            "4829153307",
            "Harbour Dental",
            "USD",
            "https://keysend.app",
        )
        .unwrap()
        .html;
        assert!(!html.contains("EEE9F6"), "{html}");
    }
}

#[test]
fn a_quiet_week_without_any_conversion_does_not_call_conversions_normal() {
    let mut rows = days(62);
    for row in rows.iter_mut().filter(|row| row.day >= 50) {
        row.metrics.conversions = Some(0.0);
    }
    let detection = detection(&rows, 0, true);
    assert!(detection.quiet());
    assert_eq!(
        update::quiet(&detection),
        "No conversion was recorded in the last 7 days, while spend and clicks stayed inside their normal range."
    );
}
