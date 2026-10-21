//! What followed an applied change, for the Campaign it changed.
//!
//! KeySend keeps three results apart: the change was sent (execution), Google Ads holds the
//! intended value (the read-back of `execute`), and what the numbers did afterwards (this
//! module). The last is an observation, never proof that the change caused it: auctions,
//! seasons, other changes and late conversions move the same numbers. It compares the settled
//! days after the change with the same number of days before it, once the Campaign has a
//! collected row for every day of both: a day that was not collected is missing data, not a
//! day without spend. It judges a count only beyond chance with both periods uncertain
//! (`stats::later`). A fractional count, from data-driven attribution, is not judged: the
//! state stays inconclusive.

use serde::{Deserialize, Serialize};

use super::{
    signals::{self, Kind},
    stats::{later, whole, EVIDENCE},
    Action, Draft,
};
use crate::{agent::data::AccountData, monitoring::metrics};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Fewer collected, settled days of the Campaign than the observation window, before or
    /// after the change.
    AwaitingData,
    /// The numbers moved the way the Proposal expected, beyond chance.
    Improving,
    /// The numbers moved against it, beyond chance.
    Deteriorating,
    /// No movement beyond chance, or the rest of the Ad Account moved the same way.
    Inconclusive,
    /// Conversion tracking is not working, so results cannot be judged.
    DataQuality,
    /// Another change of the Campaign in the window, or a change KeySend could not verify.
    ManualReview,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            Self::AwaitingData => "awaiting_data",
            Self::Improving => "improving",
            Self::Deteriorating => "deteriorating",
            Self::Inconclusive => "inconclusive",
            Self::DataQuality => "data_quality",
            Self::ManualReview => "manual_review",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Observed {
    pub state: State,
    pub days: i64,
    pub before_spend: f64,
    pub before_conversions: f64,
    pub after_spend: f64,
    pub after_conversions: f64,
    #[serde(default)]
    pub before_days: i64,
    #[serde(default)]
    pub tracking_reliable: Option<bool>,
    #[serde(default)]
    pub confounding_changes: Option<usize>,
    /// Collected, settled days of the Campaign after the change so far.
    pub settled_days: i64,
}

/// Evaluates an applied change made on `day` (the Ad Account's day number). `other_changes`
/// counts the changes of the same Campaign that Google reports in the window and that are not
/// this one.
pub fn evaluate(
    draft: &Draft,
    day: i64,
    data: &AccountData,
    window: i64,
    other_changes: usize,
) -> Observed {
    let campaign = draft.action.campaign_id();
    let (last, _) = signals::settled(data);
    let after_last = (day + window).min(last);
    let before = metrics::sum(&data.days, day - window, day - 1, Some(campaign));
    let after = metrics::sum(&data.days, day + 1, after_last, Some(campaign));
    let settled_days = collected(data, campaign, day + 1, after_last);
    let before_days = collected(data, campaign, day - window, day - 1);
    let tracking_reliable = signals::tracking_reliable(data);
    let mut observed = Observed {
        state: State::AwaitingData,
        days: window,
        before_spend: before.spend,
        before_conversions: before.conversions.unwrap_or_default(),
        after_spend: after.spend,
        after_conversions: after.conversions.unwrap_or_default(),
        before_days,
        tracking_reliable: Some(tracking_reliable),
        confounding_changes: Some(other_changes),
        settled_days,
    };
    if !tracking_reliable {
        observed.state = State::DataQuality;
        return observed;
    }
    if other_changes > 0 {
        observed.state = State::ManualReview;
        return observed;
    }
    if settled_days < window || before_days < window {
        return observed;
    }
    let rest_before = rest(data, campaign, day - window, day - 1);
    let rest_after = rest(data, campaign, day + 1, day + window);
    observed.state = match (&draft.action, draft.signal.kind) {
        (Action::AdjustCampaignBudget { .. }, Kind::CostAboveTarget) => cost(
            (observed.before_spend, observed.before_conversions),
            (observed.after_spend, observed.after_conversions),
            rest_before,
            rest_after,
        ),
        (Action::AdjustCampaignBudget { .. }, Kind::BudgetLimited) => limited(
            (observed.before_spend, observed.before_conversions),
            (observed.after_spend, observed.after_conversions),
            rest_before,
            rest_after,
        ),
        (Action::PauseCampaign { .. }, _) => paused(
            (observed.before_spend, observed.before_conversions),
            observed.after_spend,
            rest_before.1,
            rest_after.1,
        ),
        _ => State::Inconclusive,
    };
    observed
}

/// The days from `from` to `to` that have a collected row for the Campaign. A day collected
/// with no spend has a row with zeros; a day that was not collected has none, and is not
/// counted as a day without spend.
fn collected(data: &AccountData, campaign: i64, from: i64, to: i64) -> i64 {
    data.days
        .iter()
        .filter(|row| row.campaign_id == campaign && (from..=to).contains(&row.day))
        .map(|row| row.day)
        .collect::<std::collections::BTreeSet<_>>()
        .len() as i64
}

/// Spend and conversions of every other Campaign of the Ad Account.
fn rest(data: &AccountData, campaign: i64, from: i64, to: i64) -> (f64, f64) {
    let all = metrics::sum(&data.days, from, to, None);
    let one = metrics::sum(&data.days, from, to, Some(campaign));
    (
        all.spend - one.spend,
        all.conversions.unwrap_or_default() - one.conversions.unwrap_or_default(),
    )
}

/// Whether the count after is higher (or lower) than the count before beyond chance, for the
/// exposure of each period: its spend, or its days. A fractional count is never beyond chance.
fn beyond(before: (f64, f64), after: (f64, f64), more: bool) -> bool {
    let (Some(earlier), Some(later_count)) = (whole(before.1), whole(after.1)) else {
        return false;
    };
    before.0 > 0.0
        && after.0 > 0.0
        && later(earlier, later_count, after.0 / (before.0 + after.0), more) < EVIDENCE
}

/// Whether the Campaign moved beyond chance and the rest of the Ad Account did not move the
/// same way. When the rest has fractional counts its movement cannot be judged, so neither
/// can the Campaign's be told apart from it.
fn moved(
    before: (f64, f64),
    after: (f64, f64),
    rest_before: (f64, f64),
    rest_after: (f64, f64),
    more: bool,
) -> bool {
    whole(rest_before.1).is_some()
        && whole(rest_after.1).is_some()
        && beyond(before, after, more)
        && !beyond(rest_before, rest_after, more)
}

/// The cost per conversion after against before: the conversions after as a share of both
/// periods, against the share of the spend after. When the rest of the Ad Account moved the
/// same way beyond chance, the Campaign's movement cannot be told apart from it.
fn cost(
    before: (f64, f64),
    after: (f64, f64),
    rest_before: (f64, f64),
    rest_after: (f64, f64),
) -> State {
    if before.1 <= 0.0 || after.0 <= 0.0 {
        State::Inconclusive
    } else if moved(before, after, rest_before, rest_after, true) {
        State::Improving
    } else if moved(before, after, rest_before, rest_after, false) {
        State::Deteriorating
    } else {
        State::Inconclusive
    }
}

/// A raise of a budget that limited a Campaign: more conversions in the same number of days at
/// no worse a cost per conversion. Conversions that rose with the rest of the Ad Account are
/// not credited to the raise.
fn limited(
    before: (f64, f64),
    after: (f64, f64),
    rest_before: (f64, f64),
    rest_after: (f64, f64),
) -> State {
    let days = |(_, conversions): (f64, f64)| (1.0, conversions);
    let count = |more| {
        moved(
            days(before),
            days(after),
            days(rest_before),
            days(rest_after),
            more,
        )
    };
    match cost(before, after, rest_before, rest_after) {
        State::Deteriorating => State::Deteriorating,
        _ if count(true) => State::Improving,
        _ if count(false) => State::Deteriorating,
        _ => State::Inconclusive,
    }
}

/// A paused Campaign stopped spending; the question is whether the Ad Account lost
/// conversions: the rest after against the whole Ad Account before, over the same days.
fn paused(before: (f64, f64), after_spend: f64, rest_before: f64, rest_after: f64) -> State {
    let account_before = rest_before + before.1;
    if whole(account_before).is_none() || whole(rest_after).is_none() {
        State::Inconclusive
    } else if beyond((1.0, account_before), (1.0, rest_after), false) {
        State::Deteriorating
    } else if after_spend <= 0.01 * before.0.max(1.0) {
        State::Improving
    } else {
        State::Inconclusive
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimize::{
        eval::commercial::{ad, campaign, data, Line},
        signals::{Severity, Signal},
        Approval,
    };

    /// A cut of the budget of Campaign 1, applied 20 days before the last day of data, with
    /// Campaign 2 keeping the Ad Account's Conversion tracking working.
    fn observed(line: Line) -> Observed {
        let data = data(
            &ad()
                .target(100.0)
                .with(line)
                .with(campaign(2).last(60, 6000.0, 60.0)),
        );
        let draft = Draft {
            key: "adjust_campaign_budget:1".into(),
            action: Action::AdjustCampaignBudget {
                campaign_id: 1,
                budget_id: 11,
                from_micros: 200_000_000,
                to_micros: 160_000_000,
            },
            signal: Signal {
                kind: Kind::CostAboveTarget,
                campaign_id: Some(1),
                campaign: None,
                term: None,
                from_day: 0,
                to_day: 0,
                spend: 0.0,
                clicks: 0.0,
                conversions: 0.0,
                target: Some(100.0),
                chance: None,
                lost_to_budget: None,
                daily_spend: None,
                daily_budget: None,
                data_day: 0,
                severity: Severity::Low,
                caveats: vec![],
            },
            approval: Approval::Required,
            policy_version: 1,
            reasons: vec![],
            created_at: 0,
            expires_at: 0,
        };
        evaluate(&draft, data.end_day - 20, &data, 7, 0)
    }

    #[test]
    fn an_outcome_waits_for_collected_days_of_the_campaign_on_both_sides_of_the_change() {
        let missing_after = observed(campaign(1).span(60, 21, 4000.0, 40.0));
        assert_eq!(missing_after.state, State::AwaitingData);
        assert_eq!(missing_after.settled_days, 0);
        assert_eq!(
            observed(campaign(1).span(20, 0, 2000.0, 20.0)).state,
            State::AwaitingData,
            "no collected day before the change"
        );
        let zero_after = observed(campaign(1).span(60, 21, 4000.0, 40.0).span(20, 0, 0.0, 0.0));
        assert_eq!(
            (zero_after.state, zero_after.settled_days),
            (State::Inconclusive, 7),
            "collected days with no spend are days with data"
        );
        assert_eq!(zero_after.before_days, 7);
        assert_eq!(zero_after.tracking_reliable, Some(true));
        assert_eq!(zero_after.confounding_changes, Some(0));
    }

    #[test]
    fn a_cost_that_moved_with_the_rest_of_the_ad_account_is_inconclusive() {
        // 40 conversions for 1,000 before; 1,000 buys 60 after: better, beyond chance.
        assert_eq!(
            cost(
                (1000.0, 40.0),
                (1000.0, 60.0),
                (5000.0, 200.0),
                (5000.0, 200.0)
            ),
            State::Improving
        );
        assert_eq!(
            cost(
                (1000.0, 40.0),
                (1000.0, 60.0),
                (5000.0, 200.0),
                (5000.0, 300.0)
            ),
            State::Inconclusive
        );
        assert_eq!(
            cost(
                (1000.0, 40.0),
                (1000.0, 44.0),
                (5000.0, 200.0),
                (5000.0, 200.0)
            ),
            State::Inconclusive,
            "a movement within chance"
        );
        assert_eq!(
            cost(
                (1000.0, 40.0),
                (1000.0, 20.0),
                (5000.0, 200.0),
                (5000.0, 200.0)
            ),
            State::Deteriorating
        );
    }

    #[test]
    fn a_rise_within_the_chance_of_both_periods_is_inconclusive() {
        assert_eq!(
            cost(
                (1000.0, 40.0),
                (1000.0, 52.0),
                (5000.0, 200.0),
                (5000.0, 200.0)
            ),
            State::Inconclusive
        );
        assert_eq!(
            cost(
                (1000.0, 40.4),
                (1000.0, 80.2),
                (5000.0, 200.0),
                (5000.0, 200.0)
            ),
            State::Inconclusive,
            "fractional conversions"
        );
    }

    #[test]
    fn a_raise_is_credited_only_with_conversions_beyond_chance_and_beyond_the_rest_of_the_ad_account(
    ) {
        let flat = ((5000.0, 200.0), (5000.0, 200.0));
        assert_eq!(
            limited((1000.0, 40.0), (1500.0, 60.0), flat.0, flat.1),
            State::Improving
        );
        assert_eq!(
            limited(
                (1000.0, 40.0),
                (1500.0, 60.0),
                (5000.0, 200.0),
                (5000.0, 300.0)
            ),
            State::Inconclusive,
            "the rest of the Ad Account rose as much"
        );
        assert_eq!(
            limited((1000.0, 40.0), (1300.0, 52.0), flat.0, flat.1),
            State::Inconclusive,
            "within the chance of both periods"
        );
        assert_eq!(
            limited(
                (1000.0, 40.0),
                (1500.0, 60.0),
                (5000.0, 200.4),
                (5000.0, 199.7)
            ),
            State::Inconclusive,
            "the rest has fractional conversions"
        );
    }

    #[test]
    fn a_pause_is_judged_by_the_conversions_of_the_whole_ad_account() {
        assert_eq!(paused((1000.0, 40.0), 0.0, 200.0, 230.0), State::Improving);
        assert_eq!(
            paused((1000.0, 40.0), 0.0, 200.0, 200.0),
            State::Deteriorating
        );
        assert_eq!(
            paused((1000.0, 40.5), 0.0, 200.0, 230.0),
            State::Inconclusive,
            "fractional conversions"
        );
    }
}
