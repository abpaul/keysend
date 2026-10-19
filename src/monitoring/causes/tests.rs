use super::*;
use crate::monitoring::{campaign_settings::AllFollowAccount, detection::detect, metrics::Metrics};
use chrono::{Datelike, Weekday};

fn on(date: &str) -> i64 {
    day_number(NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap())
}

fn end() -> i64 {
    on("2026-10-11")
}

fn metrics(spend: f64, clicks: f64, conversions: f64) -> Metrics {
    Metrics {
        spend,
        clicks,
        impressions: clicks * 10.0,
        conversions: Some(conversions),
        conversion_value: Some(conversions * 5.0),
    }
}

fn series(campaign_id: i64, read: impl Fn(&str) -> Metrics) -> Vec<CampaignDay> {
    (end() - 62..=end())
        .map(|day| CampaignDay {
            day,
            campaign_id,
            metrics: read(&date_of(day).unwrap().to_string()),
        })
        .collect()
}

fn quiet(campaign_id: i64) -> Vec<CampaignDay> {
    series(campaign_id, |_| metrics(10.0, 10.0, 1.0))
}

fn campaigns() -> Vec<Campaign> {
    [(701, "Search - Real"), (702, "Brand")]
        .map(|(id, name)| Campaign {
            id,
            name: name.into(),
            status: "ENABLED".into(),
            channel_type: "SEARCH".into(),
            bidding_strategy: "MAXIMIZE_CONVERSIONS".into(),
            budget_micros: None,
        })
        .into()
}

fn change(campaign_id: Option<i64>, date: &str, old: &str, new: &str) -> AccountChange {
    AccountChange {
        changed_at: format!("{date} 10:00:00"),
        resource_name: "customers/1/changeEvents/1".into(),
        resource_type: "CAMPAIGN".into(),
        operation: "UPDATE".into(),
        campaign_id,
        changed_fields: String::new(),
        old_value: old.into(),
        new_value: new.into(),
    }
}

fn budget(campaign_id: Option<i64>, date: &str, old: i64, new: i64) -> AccountChange {
    let wrap = |micros: i64| format!(r#"{{"campaignBudget":{{"amountMicros":"{micros}"}}}}"#);
    change(campaign_id, date, &wrap(old), &wrap(new))
}

fn bid_strategy(date: &str) -> AccountChange {
    change(
        Some(701),
        date,
        r#"{"campaign":{"biddingStrategyType":"TARGET_CPA"}}"#,
        r#"{"campaign":{"biddingStrategyType":"MAXIMIZE_CONVERSIONS"}}"#,
    )
}

fn inputs<'a>(days: &'a [CampaignDay], changes: &'a [AccountChange]) -> Inputs<'a> {
    Inputs {
        days,
        changes,
        names: campaigns()
            .into_iter()
            .map(|campaign| (campaign.id, campaign.name))
            .collect(),
        shares_now: vec![],
        shares_before: vec![],
        devices: BTreeMap::new(),
    }
}

fn found_with(days: &[CampaignDay], changes: &[AccountChange], inputs: Inputs) -> Vec<Candidate> {
    let detection = detect(
        days,
        &campaigns(),
        changes,
        end(),
        end() - 62,
        AllFollowAccount { judged: true },
        "USD",
    );
    candidates(&detection, &inputs)
}

fn found(days: &[CampaignDay], changes: &[AccountChange]) -> Vec<Candidate> {
    found_with(days, changes, inputs(days, changes))
}

fn of_kind(found: &[Candidate], kind: Kind, campaign: Option<i64>) -> Vec<&Candidate> {
    found
        .iter()
        .filter(|candidate| candidate.kind == kind && candidate.campaign_id == campaign)
        .collect()
}

fn both(first: Vec<CampaignDay>, second: Vec<CampaignDay>) -> Vec<CampaignDay> {
    first.into_iter().chain(second).collect()
}

fn spend_rise_from(date: &'static str) -> Vec<CampaignDay> {
    series(701, move |day| {
        metrics(if day >= date { 20.0 } else { 10.0 }, 10.0, 1.0)
    })
}

#[test]
fn a_budget_raised_before_the_spend_rose_is_a_confirmed_cause_of_the_campaign() {
    let days = both(spend_rise_from("2026-10-08"), quiet(702));
    let changes = [budget(Some(701), "2026-10-07", 10_000_000, 30_000_000)];
    let found = found(&days, &changes);
    let causes = of_kind(&found, Kind::AccountChange, Some(701));
    assert_eq!(causes.len(), 1);
    assert_eq!(causes[0].id, "701/account-change-budget/spend/2026-10-07");
    assert_eq!(causes[0].level, Level::Confirmed);
    assert_eq!(causes[0].metric, MetricName::Spend);
    assert_eq!(
        causes[0].evidence,
        "Daily budget changed from $10.00 to $30.00 on 7 October. Spend averaged $20.00 a day after it, against $10.00 a day on the same weekdays of the week before."
    );
}

#[test]
fn a_budget_raise_that_the_spend_never_reached_is_only_a_likely_cause() {
    let days = both(spend_rise_from("2026-10-08"), quiet(702));
    let changes = [budget(Some(701), "2026-10-07", 50_000_000, 80_000_000)];
    let found = found(&days, &changes);
    let causes = of_kind(&found, Kind::AccountChange, Some(701));
    assert_eq!(causes.len(), 1);
    assert_eq!(causes[0].level, Level::Likely);
}

#[test]
fn a_budget_cut_is_confirmed_only_when_the_spend_after_it_reaches_the_new_budget() {
    let fall = both(
        series(701, |day| {
            metrics(if day >= "2026-10-08" { 10.0 } else { 20.0 }, 10.0, 1.0)
        }),
        quiet(702),
    );
    for (new, level) in [(10_000_000, Level::Confirmed), (40_000_000, Level::Likely)] {
        let changes = [budget(Some(701), "2026-10-07", 80_000_000, new)];
        let found = found(&fall, &changes);
        let causes = of_kind(&found, Kind::AccountChange, Some(701));
        assert_eq!(causes.len(), 1, "{new}");
        assert_eq!(causes[0].level, level, "{new}");
    }
}

#[test]
fn a_pause_before_a_fall_is_a_confirmed_cause_without_a_budget_check() {
    let fall = both(
        series(701, |day| {
            metrics(if day >= "2026-10-08" { 0.0 } else { 10.0 }, 10.0, 1.0)
        }),
        quiet(702),
    );
    let changes = [change(
        Some(701),
        "2026-10-07",
        r#"{"campaign":{"status":"ENABLED"}}"#,
        r#"{"campaign":{"status":"PAUSED"}}"#,
    )];
    let found = found(&fall, &changes);
    let causes = of_kind(&found, Kind::AccountChange, Some(701));
    assert_eq!(causes.len(), 1);
    assert_eq!(causes[0].level, Level::Confirmed);
}

#[test]
fn a_weekend_after_a_friday_change_is_not_a_fall_that_the_change_caused() {
    let weekend_pattern = |first_weekend: f64| {
        both(
            series(701, move |day| {
                let date = NaiveDate::parse_from_str(day, "%Y-%m-%d").unwrap();
                let spend = match date.weekday() {
                    Weekday::Sat | Weekday::Sun if day >= "2026-10-10" => first_weekend,
                    Weekday::Sat | Weekday::Sun => 2.0,
                    _ if day >= "2026-10-05" && first_weekend == 2.0 => 6.0,
                    _ => 10.0,
                };
                metrics(spend, 10.0, 1.0)
            }),
            quiet(702),
        )
    };
    let changes = [budget(Some(701), "2026-10-09", 80_000_000, 50_000_000)];
    let usual_weekend = weekend_pattern(2.0);
    assert!(of_kind(
        &found(&usual_weekend, &changes),
        Kind::AccountChange,
        Some(701)
    )
    .is_empty());
    let weaker_weekend = weekend_pattern(0.5);
    assert_eq!(
        of_kind(
            &found(&weaker_weekend, &changes),
            Kind::AccountChange,
            Some(701)
        )
        .len(),
        1
    );
}

#[test]
fn the_ad_account_names_the_campaign_that_explains_most_of_its_movement() {
    let days = both(spend_rise_from("2026-10-08"), quiet(702));
    let changes = [budget(Some(701), "2026-10-07", 10_000_000, 30_000_000)];
    let found = found(&days, &changes);
    let causes = of_kind(&found, Kind::AccountChange, None);
    assert_eq!(causes.len(), 1);
    assert_eq!(
        causes[0].id,
        "account/701/account-change-budget/spend/2026-10-07"
    );
    assert_eq!(
        causes[0].evidence,
        "Search - Real: Daily budget changed from $10.00 to $30.00 on 7 October. Spend averaged $20.00 a day after it, against $10.00 a day on the same weekdays of the week before. It accounts for 100% of the rise in Spend."
    );
    assert!(of_kind(&found, Kind::Concentration, None).is_empty());
}

#[test]
fn a_change_in_the_wrong_direction_explains_nothing() {
    let days = both(spend_rise_from("2026-10-08"), quiet(702));
    let changes = [budget(Some(701), "2026-10-07", 80_000_000, 50_000_000)];
    assert!(of_kind(&found(&days, &changes), Kind::AccountChange, Some(701)).is_empty());
}

#[test]
fn a_change_after_the_movement_explains_nothing() {
    let days = both(spend_rise_from("2026-10-05"), quiet(702));
    for date in ["2026-10-08", "2026-10-10", "2026-10-11"] {
        let changes = [budget(Some(701), date, 50_000_000, 80_000_000)];
        assert!(
            of_kind(&found(&days, &changes), Kind::AccountChange, Some(701)).is_empty(),
            "{date}"
        );
    }
}

#[test]
fn a_change_before_the_window_explains_nothing() {
    let days = both(spend_rise_from("2026-10-08"), quiet(702));
    let changes = [budget(Some(701), "2026-10-03", 50_000_000, 80_000_000)];
    assert!(of_kind(&found(&days, &changes), Kind::AccountChange, Some(701)).is_empty());
}

#[test]
fn a_change_to_the_ad_account_itself_is_a_cause_of_the_ad_account() {
    let days = both(spend_rise_from("2026-10-08"), quiet(702));
    let changes = [budget(None, "2026-10-07", 50_000_000, 80_000_000)];
    let found = found(&days, &changes);
    let causes = of_kind(&found, Kind::AccountChange, None);
    assert_eq!(causes.len(), 1);
    assert_eq!(
        causes[0].id,
        "account/account-change-budget/spend/2026-10-07"
    );
    assert_eq!(causes[0].level, Level::Likely);
    assert_eq!(
        causes[0].evidence,
        "Daily budget changed from $50.00 to $80.00 on 7 October. Spend averaged $30.00 a day after it, against $20.00 a day on the same weekdays of the week before."
    );
}

fn conversion_rise() -> Vec<CampaignDay> {
    both(
        series(701, |day| {
            metrics(10.0, 10.0, if day >= "2026-10-07" { 3.0 } else { 1.0 })
        }),
        quiet(702),
    )
}

#[test]
fn a_new_bid_strategy_before_a_conversion_rise_is_only_a_likely_cause() {
    let days = conversion_rise();
    let changes = [bid_strategy("2026-10-06")];
    let found = found(&days, &changes);
    let causes = of_kind(&found, Kind::AccountChange, Some(701));
    assert_eq!(causes.len(), 1);
    assert_eq!(
        causes[0].id,
        "701/account-change-bid-strategy/conversions/2026-10-06"
    );
    assert_eq!(causes[0].level, Level::Likely);
    assert_eq!(
        causes[0].evidence,
        "Bid strategy changed from target cpa to maximize conversions on 6 October. Conversions averaged 3 a day after it, against 1 a day on the same weekdays of the week before."
    );
}

#[test]
fn a_budget_raise_is_only_a_likely_cause_of_a_conversion_rise() {
    let days = conversion_rise();
    let changes = [budget(Some(701), "2026-10-06", 50_000_000, 80_000_000)];
    let found = found(&days, &changes);
    let causes = of_kind(&found, Kind::AccountChange, Some(701));
    assert_eq!(causes.len(), 1);
    assert_eq!(causes[0].level, Level::Likely);
    assert_eq!(causes[0].metric, MetricName::Conversions);
}

fn conversions_stop(before: f64, clicks_before: f64, clicks_after: f64) -> Vec<Candidate> {
    let days = series(701, |day| {
        if day <= "2026-10-03" {
            metrics(10.0, clicks_before, before)
        } else {
            metrics(10.0, clicks_after, 0.0)
        }
    });
    found(&days, &[])
}

#[test]
fn conversions_that_stop_while_clicks_hold_are_a_likely_tracking_problem() {
    let found = conversions_stop(2.0, 20.0, 20.0);
    for campaign in [None, Some(701)] {
        let causes = of_kind(&found, Kind::Tracking, campaign);
        assert_eq!(causes.len(), 1, "{campaign:?}");
        assert_eq!(causes[0].level, Level::Likely);
        assert_eq!(causes[0].metric, MetricName::Conversions);
        assert_eq!(
            causes[0].evidence,
            "Conversions fell from 14 to 0 in the 7 days that ended 2 days ago, while Clicks stayed inside their normal range at 140 in the last 7 days."
        );
    }
    assert_eq!(found[0].kind, Kind::Tracking);
}

#[test]
fn conversions_that_jump_while_clicks_hold_are_a_likely_tracking_problem_too() {
    let days = series(701, |day| {
        if day <= "2026-10-03" {
            metrics(10.0, 20.0, 2.0)
        } else {
            metrics(10.0, 20.0, 5.0)
        }
    });
    let found = found(&days, &[]);
    let causes = of_kind(&found, Kind::Tracking, None);
    assert_eq!(causes.len(), 1);
    assert_eq!(
        causes[0].evidence,
        "Conversions rose from 14 to 35 in the 7 days that ended 2 days ago, while Clicks stayed inside their normal range at 140 in the last 7 days."
    );
    assert_eq!(
        statement(causes[0], Some(true)).unwrap(),
        "Conversions rose, most likely because conversion tracking counts some of them twice."
    );
}

#[test]
fn a_small_count_or_falling_clicks_or_few_clicks_are_no_tracking_problem() {
    assert!(
        of_kind(&conversions_stop(1.0, 20.0, 20.0), Kind::Tracking, None).is_empty(),
        "7 conversions before are too few"
    );
    assert!(
        of_kind(&conversions_stop(2.0, 20.0, 5.0), Kind::Tracking, None).is_empty(),
        "clicks fell too"
    );
    assert!(
        of_kind(&conversions_stop(2.0, 10.0, 10.0), Kind::Tracking, None).is_empty(),
        "70 clicks are too few"
    );
}

#[test]
fn a_campaign_that_stopped_serving_is_a_confirmed_cause_of_its_spend_fall() {
    let days = both(
        series(701, |day| {
            if day >= "2026-10-08" {
                metrics(0.0, 0.0, 0.0)
            } else {
                metrics(10.0, 10.0, 1.0)
            }
        }),
        quiet(702),
    );
    let found = found(&days, &[]);
    let own = of_kind(&found, Kind::CampaignStopped, Some(701));
    assert_eq!(own.len(), 1);
    assert_eq!(own[0].id, "701/stopped/spend");
    assert_eq!(own[0].level, Level::Confirmed);
    assert_eq!(
        own[0].evidence,
        "Campaign stopped serving on 8 October, and Spend was $30.00 in the last 7 days, against $70.00 in the previous 7 days."
    );
    let account = of_kind(&found, Kind::CampaignStopped, None);
    assert_eq!(account.len(), 1);
    assert_eq!(
        account[0].evidence,
        "Search - Real: Campaign stopped serving on 8 October, and Spend was $30.00 in the last 7 days, against $70.00 in the previous 7 days. It accounts for 100% of the fall in Spend."
    );
}

#[test]
fn a_campaign_that_started_serving_is_a_confirmed_cause_of_its_spend_rise() {
    let days = both(
        series(701, |day| {
            if day >= "2026-10-06" {
                metrics(10.0, 10.0, 1.0)
            } else {
                metrics(0.0, 0.0, 0.0)
            }
        }),
        quiet(702),
    );
    let found = found(&days, &[]);
    let own = of_kind(&found, Kind::CampaignStarted, Some(701));
    assert_eq!(own.len(), 1);
    assert_eq!(own[0].id, "701/started/spend");
    assert_eq!(own[0].level, Level::Confirmed);
    assert_eq!(
        own[0].evidence,
        "Campaign started serving in the last 7 days, and Spend was $60.00, against $0.00 in the previous 7 days."
    );
}

fn click_fall() -> Vec<CampaignDay> {
    both(
        series(701, |day| {
            metrics(10.0, if day >= "2026-10-05" { 5.0 } else { 10.0 }, 1.0)
        }),
        quiet(702),
    )
}

fn shares(campaign_id: i64, lost_budget: f64, lost_rank: f64, limit_days: i64) -> SearchShareTotal {
    SearchShareTotal {
        campaign_id,
        shares: Some(SearchShares {
            shown: 0.6,
            lost_budget,
            lost_rank,
        }),
        limit_days,
        exact_days: 7,
        days: 7,
    }
}

fn with_shares(now: SearchShareTotal, before: SearchShareTotal) -> Vec<Candidate> {
    let days = click_fall();
    let mut inputs = inputs(&days, &[]);
    inputs.shares_now = vec![now];
    inputs.shares_before = vec![before];
    found_with(&days, &[], inputs)
}

#[test]
fn a_clear_rise_in_the_share_lost_to_budget_is_a_likely_cause_of_fewer_clicks() {
    let found = with_shares(shares(701, 0.34, 0.02, 0), shares(701, 0.12, 0.02, 0));
    let causes = of_kind(&found, Kind::BudgetLimit, Some(701));
    assert_eq!(causes.len(), 1);
    assert_eq!(causes[0].id, "701/budget-limit/clicks");
    assert_eq!(causes[0].level, Level::Likely);
    assert_eq!(causes[0].metric, MetricName::Clicks);
    assert_eq!(
        causes[0].evidence,
        "Share of impressions lost to budget was 34% in the last 7 days, against 12% in the previous 7 days."
    );
    assert!(of_kind(&found, Kind::RankLost, Some(701)).is_empty());
}

#[test]
fn a_clear_rise_in_the_share_lost_to_rank_is_a_likely_cause_of_fewer_clicks() {
    let found = with_shares(shares(701, 0.02, 0.30, 0), shares(701, 0.02, 0.05, 0));
    let causes = of_kind(&found, Kind::RankLost, Some(701));
    assert_eq!(causes.len(), 1);
    assert_eq!(causes[0].level, Level::Likely);
    assert_eq!(
        causes[0].evidence,
        "Share of impressions lost to rank was 30% in the last 7 days, against 5% in the previous 7 days."
    );
    assert!(of_kind(&found, Kind::BudgetLimit, Some(701)).is_empty());
}

#[test]
fn a_share_that_is_not_exact_or_not_clearly_higher_is_no_cause() {
    let unknown = SearchShareTotal {
        shares: None,
        ..shares(701, 0.0, 0.0, 0)
    };
    for (now, before) in [
        (unknown.clone(), shares(701, 0.12, 0.02, 0)),
        (shares(701, 0.34, 0.02, 0), unknown),
        (shares(701, 0.34, 0.02, 1), shares(701, 0.12, 0.02, 0)),
        (shares(701, 0.34, 0.02, 0), shares(701, 0.12, 0.02, 2)),
        (shares(701, 0.34, 0.02, 0), shares(701, 0.30, 0.02, 0)),
        (shares(701, 0.15, 0.02, 0), shares(701, 0.02, 0.02, 0)),
        (
            SearchShareTotal {
                exact_days: 4,
                ..shares(701, 0.34, 0.02, 0)
            },
            shares(701, 0.12, 0.02, 0),
        ),
        (shares(702, 0.34, 0.02, 0), shares(702, 0.12, 0.02, 0)),
    ] {
        let found = with_shares(now.clone(), before.clone());
        assert!(
            of_kind(&found, Kind::BudgetLimit, Some(701)).is_empty(),
            "{now:?} {before:?}"
        );
    }
}

#[test]
fn a_share_lost_to_budget_does_not_explain_clicks_that_rose() {
    let days = both(
        series(701, |day| {
            metrics(10.0, if day >= "2026-10-05" { 20.0 } else { 10.0 }, 1.0)
        }),
        quiet(702),
    );
    let mut inputs = inputs(&days, &[]);
    inputs.shares_now = vec![shares(701, 0.34, 0.02, 0)];
    inputs.shares_before = vec![shares(701, 0.12, 0.02, 0)];
    let found = found_with(&days, &[], inputs);
    assert!(of_kind(&found, Kind::BudgetLimit, Some(701)).is_empty());
}

fn spend_rise_of(first: f64, second: f64) -> Vec<CampaignDay> {
    both(
        series(701, |day| {
            metrics(if day >= "2026-10-05" { first } else { 10.0 }, 10.0, 1.0)
        }),
        series(702, |day| {
            metrics(if day >= "2026-10-05" { second } else { 10.0 }, 10.0, 1.0)
        }),
    )
}

#[test]
fn the_campaign_that_gives_most_of_the_ad_account_movement_is_a_contribution_not_a_cause() {
    let found = found(&spend_rise_of(30.0, 15.0), &[]);
    let causes = of_kind(&found, Kind::Concentration, None);
    assert_eq!(causes.len(), 1);
    assert_eq!(causes[0].id, "account/concentration/spend/701");
    assert_eq!(causes[0].level, Level::Contribution);
    assert!(causes[0].anchors.is_empty());
    assert_eq!(
        causes[0].evidence,
        "Search - Real accounts for 80% of the rise in Spend: $140 of $175."
    );
}

#[test]
fn two_campaigns_with_equal_parts_or_one_campaign_alone_are_no_concentration() {
    assert!(of_kind(
        &found(&spend_rise_of(20.0, 20.0), &[]),
        Kind::Concentration,
        None
    )
    .is_empty());
    let alone = series(701, |day| {
        metrics(if day >= "2026-10-05" { 20.0 } else { 10.0 }, 10.0, 1.0)
    });
    assert!(of_kind(&found(&alone, &[]), Kind::Concentration, None).is_empty());
}

fn device(device: i64, spend: f64) -> DeviceTotal {
    DeviceTotal {
        device,
        spend,
        clicks: 0.0,
        conversions: 0.0,
    }
}

fn with_devices(now: Vec<DeviceTotal>, before: Vec<DeviceTotal>) -> Vec<Candidate> {
    let days = both(spend_rise_from("2026-10-05"), quiet(702));
    let mut inputs = inputs(&days, &[]);
    inputs.devices.insert((701, false), (now, before));
    found_with(&days, &[], inputs)
}

#[test]
fn the_device_that_gives_most_of_a_campaign_movement_is_a_contribution_not_a_cause() {
    let found = with_devices(
        vec![device(1, 100.0), device(2, 40.0)],
        vec![device(1, 40.0), device(2, 30.0)],
    );
    let causes = of_kind(&found, Kind::Concentration, Some(701));
    assert_eq!(causes.len(), 1);
    assert_eq!(causes[0].id, "701/concentration/spend/device-1");
    assert_eq!(causes[0].level, Level::Contribution);
    assert!(causes[0].anchors.is_empty());
    assert_eq!(
        causes[0].evidence,
        "Mobile accounts for 86% of the rise in Spend: $60.00 of $70.00."
    );
}

#[test]
fn devices_with_equal_parts_or_one_device_alone_are_no_concentration() {
    for (now, before) in [
        (
            vec![device(1, 75.0), device(2, 65.0)],
            vec![device(1, 40.0), device(2, 30.0)],
        ),
        (vec![device(1, 140.0)], vec![device(1, 70.0)]),
    ] {
        assert!(of_kind(&with_devices(now, before), Kind::Concentration, Some(701)).is_empty());
    }
}

#[test]
fn candidates_come_in_the_order_that_an_analyst_checks_them() {
    let days = both(spend_rise_from("2026-10-08"), quiet(702));
    let changes = [budget(Some(701), "2026-10-07", 10_000_000, 30_000_000)];
    let mut inputs = inputs(&days, &changes);
    inputs.devices.insert(
        (701, false),
        (
            vec![device(1, 100.0), device(2, 40.0)],
            vec![device(1, 40.0), device(2, 30.0)],
        ),
    );
    let found = found_with(&days, &changes, inputs);
    let kinds = found
        .iter()
        .filter(|candidate| candidate.campaign_id == Some(701))
        .map(|candidate| candidate.kind)
        .collect::<Vec<_>>();
    assert_eq!(kinds, [Kind::AccountChange, Kind::Concentration]);
}

#[test]
fn a_quiet_ad_account_has_no_cause() {
    let days = both(quiet(701), quiet(702));
    assert_eq!(found(&days, &[]), vec![]);
}
