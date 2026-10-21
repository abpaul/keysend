//! Signals: what code detects in the Account history that could justify a change. Each
//! Signal states its Metric, its days, its values, the count behind it, how fresh the data
//! is and its caveats. A Signal is not a decision; `propose` decides.
//!
//! Only settled days count: the last day is left out, and the last week when a quarter or
//! more of the Ad Account's conversions arrive two days or more after the click. Only a whole
//! count of conversions is judged: where a rule would judge a fractional count, from
//! data-driven attribution, the Signal says so (`ConversionsNotWhole`) instead of rounding it.

use serde::{Deserialize, Serialize};

use super::{
    policy::Policy,
    stats::{poisson_at_most, whole, EVIDENCE},
};
use crate::{
    agent::data::AccountData,
    monitoring::{
        metrics::{self, last_settled_conversion_day},
        tracking::ConversionTracking,
    },
};

/// A cost per conversion enters a Signal at this multiple of the target and leaves it below
/// `CLEAR`: the gap between them keeps a Campaign near its target from being proposed one
/// week and cleared the next.
pub const ENTER: f64 = 1.3;
pub const CLEAR: f64 = 1.1;
/// Days of the window that a Signal judges.
pub const WINDOW_DAYS: i64 = 30;
/// A Campaign that lost this share of its Search impressions to budget is limited by it.
const LOST_TO_BUDGET: f64 = 0.1;
/// A budget binds when the average daily spend of the last 7 settled days reaches this share
/// of it.
pub const BINDING: f64 = 0.9;
/// Clicks a Campaign or a search term needs before its zero conversions count.
const CLICKS: f64 = 30.0;
/// Search terms that one Campaign may propose to exclude in one evaluation.
const TERMS: usize = 3;
/// The share of conversions arriving two days or more after the click above which the last
/// week is not settled.
const LATE_SHARE: f64 = 0.25;
const LATE_DAYS: i64 = 7;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// The Campaign's cost per conversion is above the target, beyond chance, and the
    /// profile's economics do not show it paying for itself.
    CostAboveTarget,
    /// The Campaign spent more than three target costs per conversion with no conversion.
    SpendWithoutConversions,
    /// The Campaign is limited by its budget and converts at or below the target.
    BudgetLimited,
    /// A search term spent more than three target costs per conversion with no conversion.
    SearchTermWithoutConversions,
    /// Conversion tracking stopped, spiked, or is not working: no change of spend is judged.
    TrackingUnreliable,
    /// A rule would judge the Campaign, but its conversions are a fractional count, as
    /// data-driven attribution credits them: no count test applies, so no change is judged.
    ConversionsNotWhole,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Low,
    Medium,
    High,
}

/// A fact that limits what a Signal can show. Each is shown with the Proposal.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Caveat {
    /// Recent days left out because their conversions may still arrive.
    UnsettledDays { days: i64 },
    /// Google Ads sets the bids of the Campaign; a budget change interacts with its learning.
    AutomatedBidding { strategy: String },
    /// The search terms come from the latest Deep check, a snapshot of the top terms.
    DeepCheckSnapshot { day: i64 },
    /// Zero recorded conversions do not prove that a term or a Campaign brings none.
    ZeroIsNotProof,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Signal {
    pub kind: Kind,
    pub campaign_id: Option<i64>,
    pub campaign: Option<String>,
    /// The search term, for a search-term Signal.
    pub term: Option<String>,
    /// The first and last settled day of the window.
    pub from_day: i64,
    pub to_day: i64,
    pub spend: f64,
    pub clicks: f64,
    pub conversions: f64,
    /// The target cost per conversion that the Signal judged against.
    pub target: Option<f64>,
    /// The chance of a count this far from the target if the Campaign converted at the
    /// target. Small values are evidence.
    pub chance: Option<f64>,
    /// The share of Search impressions lost to budget.
    pub lost_to_budget: Option<f64>,
    /// The average daily spend of the last 7 settled days and the daily budget.
    pub daily_spend: Option<f64>,
    pub daily_budget: Option<f64>,
    /// The newest day of the Account history.
    pub data_day: i64,
    pub severity: Severity,
    pub caveats: Vec<Caveat>,
}

/// The cost per conversion that Optimize judges against: the policy's, the profile's target,
/// or the break-even cost that the profile's economics give.
pub fn target(data: &AccountData, policy: &Policy) -> Option<f64> {
    policy
        .target_cost_micros
        .map(|micros| micros as f64 / 1_000_000.0)
        .or(data.profile.target_cost)
        .or_else(|| data.profile.economics.break_even_cost())
        .filter(|target| *target > 0.0)
}

/// The last settled day, with the days left out.
pub fn settled(data: &AccountData) -> (i64, i64) {
    let late = data
        .conversion_delay
        .is_some_and(|(total, late)| total > 0.0 && late / total >= LATE_SHARE);
    let last = if late {
        data.end_day - LATE_DAYS
    } else {
        last_settled_conversion_day(data.end_day)
    };
    (last, data.end_day - last)
}

fn severity(spend: f64, target: f64) -> Severity {
    let multiple = spend / target;
    if multiple >= 10.0 {
        Severity::High
    } else if multiple >= 4.0 {
        Severity::Medium
    } else {
        Severity::Low
    }
}

/// Whether Conversion tracking can be trusted for a decision about spend.
pub fn tracking_reliable(data: &AccountData) -> bool {
    data.rules.account().state == ConversionTracking::Working
        && !data.findings.iter().any(|finding| {
            matches!(
                finding.rule.key.as_str(),
                "tracking-stopped" | "tracking-spike" | "tracking-not-set-up"
            )
        })
}

pub fn detect(data: &AccountData, policy: &Policy) -> Vec<Signal> {
    let (last, unsettled) = settled(data);
    let first = last - WINDOW_DAYS + 1;
    let data_day = data
        .days
        .iter()
        .map(|day| day.day)
        .max()
        .unwrap_or(i64::MIN);
    let base = |kind, severity| Signal {
        kind,
        campaign_id: None,
        campaign: None,
        term: None,
        from_day: first,
        to_day: last,
        spend: 0.0,
        clicks: 0.0,
        conversions: 0.0,
        target: None,
        chance: None,
        lost_to_budget: None,
        daily_spend: None,
        daily_budget: None,
        data_day,
        severity,
        caveats: vec![Caveat::UnsettledDays { days: unsettled }],
    };
    if !tracking_reliable(data) {
        let total = metrics::sum(&data.days, first, last, None);
        return vec![Signal {
            spend: total.spend,
            clicks: total.clicks,
            conversions: total.conversions.unwrap_or_default(),
            ..base(Kind::TrackingUnreliable, Severity::High)
        }];
    }
    let Some(target) = target(data, policy) else {
        return Vec::new();
    };
    let mut signals = Vec::new();
    for campaign in data
        .campaigns
        .iter()
        .filter(|campaign| campaign.status == "ENABLED")
    {
        let counts = metrics::sum(&data.days, first, last, Some(campaign.id));
        let recent = metrics::sum(&data.days, last - 6, last, Some(campaign.id));
        let conversions = counts.conversions.unwrap_or_default();
        let mut caveats = vec![Caveat::UnsettledDays { days: unsettled }];
        if automated(&campaign.bidding_strategy) {
            caveats.push(Caveat::AutomatedBidding {
                strategy: campaign.bidding_strategy.clone(),
            });
        }
        let signal = |kind| Signal {
            campaign_id: Some(campaign.id),
            campaign: Some(campaign.name.clone()),
            spend: counts.spend,
            clicks: counts.clicks,
            conversions,
            target: Some(target),
            daily_spend: Some(recent.spend / 7.0),
            daily_budget: campaign
                .budget_micros
                .map(|micros| micros as f64 / 1_000_000.0),
            caveats: caveats.clone(),
            ..base(kind, severity(counts.spend, target))
        };
        let expected = counts.spend / target;
        let count = whole(conversions);
        let judged = count.filter(|count| *count >= u64::from(policy.min_conversions));
        let lost = data.deep_check.as_ref().and_then(|check| {
            check
                .data
                .audit
                .campaigns
                .iter()
                .find(|row| row.id == Some(campaign.id))
                .and_then(|row| row.lost_to_budget)
        });
        let binding = campaign
            .budget_micros
            .is_some_and(|micros| recent.spend / 7.0 >= BINDING * micros as f64 / 1_000_000.0);
        let limited = lost.is_some_and(|lost| lost >= LOST_TO_BUDGET) && binding;
        let cost = counts.spend / conversions;
        let unjudged = if conversions < 1.0 {
            counts.clicks >= CLICKS && (-expected).exp() < EVIDENCE
        } else {
            conversions >= f64::from(policy.min_conversions)
                && (cost >= target * ENTER || (cost <= target / CLEAR && limited))
        };
        if count.is_none() && unjudged {
            signals.push(signal(Kind::ConversionsNotWhole));
        } else if count == Some(0) {
            let chance = (-expected).exp();
            if counts.clicks >= CLICKS && chance < EVIDENCE {
                let mut found = signal(Kind::SpendWithoutConversions);
                found.chance = Some(chance);
                found.caveats.push(Caveat::ZeroIsNotProof);
                signals.push(found);
            }
        } else if let Some(count) = judged.filter(|_| cost >= target * ENTER) {
            let chance = poisson_at_most(count, expected);
            if chance < EVIDENCE && !pays(data, &counts) {
                let mut found = signal(Kind::CostAboveTarget);
                found.chance = Some(chance);
                signals.push(found);
            }
        } else if let Some(count) = judged.filter(|_| cost <= target / CLEAR) {
            // Fewer conversions at the target than recorded, beyond chance: the cost is
            // below the target.
            let chance = 1.0 - poisson_at_most(count.saturating_sub(1), expected);
            if limited && chance < EVIDENCE {
                let mut found = signal(Kind::BudgetLimited);
                found.chance = Some(chance);
                found.lost_to_budget = lost;
                signals.push(found);
            }
        }
        if let Some(check) = &data.deep_check {
            let mut terms = check
                .by_campaign
                .get(&campaign.id)
                .map(|audit| {
                    audit
                        .search_terms
                        .iter()
                        .filter(|row| row.counts.conversions <= 0.0 && row.counts.clicks >= CLICKS)
                        .filter(|row| (-row.counts.cost / target).exp() < EVIDENCE)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            terms.sort_by(|a, b| b.counts.cost.total_cmp(&a.counts.cost));
            for row in terms.into_iter().take(TERMS) {
                signals.push(Signal {
                    term: Some(row.name.clone()),
                    spend: row.counts.cost,
                    clicks: row.counts.clicks,
                    conversions: 0.0,
                    chance: Some((-row.counts.cost / target).exp()),
                    from_day: check.day - WINDOW_DAYS,
                    to_day: check.day - 1,
                    caveats: vec![
                        Caveat::DeepCheckSnapshot { day: check.day },
                        Caveat::ZeroIsNotProof,
                    ],
                    severity: severity(row.counts.cost, target),
                    ..signal(Kind::SearchTermWithoutConversions)
                });
            }
        }
    }
    signals
}

/// Whether the profile's own economics show a Campaign's conversions paying for their cost:
/// its cost per conversion is within the break-even cost of a conversion. A cost above the
/// target is then a conflict between the target and the economics, not evidence of a loss,
/// and a cut could lose profitable conversions. Recorded conversion value is not used: revenue
/// without a margin and the costs after it does not show a profit.
pub fn pays(data: &AccountData, counts: &metrics::Metrics) -> bool {
    let conversions = counts.conversions.unwrap_or_default();
    data.profile
        .economics
        .break_even_cost()
        .is_some_and(|cost| conversions > 0.0 && counts.spend / conversions <= cost)
}

/// Bid strategies where Google Ads sets the bids.
pub fn automated(strategy: &str) -> bool {
    matches!(
        strategy,
        "MAXIMIZE_CONVERSIONS"
            | "MAXIMIZE_CONVERSION_VALUE"
            | "TARGET_CPA"
            | "TARGET_ROAS"
            | "TARGET_SPEND"
            | "TARGET_IMPRESSION_SHARE"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimize::eval::commercial::{ad, campaign, data, Account};

    fn found(account: Account) -> Vec<(Kind, f64)> {
        detect(&data(&account), &Policy::default())
            .into_iter()
            .filter(|signal| signal.campaign_id == Some(1))
            .map(|signal| (signal.kind, (signal.conversions * 100.0).round() / 100.0))
            .collect()
    }

    /// A second Campaign keeps the Ad Account's Conversion tracking working.
    fn tracked(account: Account) -> Account {
        account.with(campaign(2).last(30, 3000.0, 40.0))
    }

    #[test]
    fn a_campaign_with_a_fraction_of_a_conversion_is_not_described_as_without_conversions() {
        assert_eq!(
            found(tracked(
                ad().target(100.0).with(campaign(1).last(30, 3000.0, 0.4))
            )),
            [(Kind::ConversionsNotWhole, 0.39)]
        );
        assert_eq!(
            found(tracked(
                ad().target(100.0).with(campaign(1).last(30, 3000.0, 0.0))
            )),
            [(Kind::SpendWithoutConversions, 0.0)]
        );
    }

    #[test]
    fn a_fractional_count_of_conversions_is_not_judged_as_a_whole_count() {
        assert_eq!(
            found(tracked(
                ad().target(100.0).with(campaign(1).last(31, 6200.0, 20.5))
            )),
            [(Kind::ConversionsNotWhole, 19.84)]
        );
        assert_eq!(
            found(tracked(
                ad().target(100.0).with(campaign(1).last(31, 6200.0, 31.0))
            )),
            [(Kind::CostAboveTarget, 30.0)]
        );
        assert_eq!(
            found(tracked(
                ad().target(100.0).with(campaign(1).last(31, 3100.0, 30.5))
            )),
            [],
            "a fractional count on target gives no Signal, as a whole one would"
        );
    }

    #[test]
    fn a_cost_above_the_target_is_no_signal_when_the_profiles_economics_show_a_profit() {
        let costly = || campaign(1).last(31, 6200.0, 31.0);
        assert_eq!(
            found(tracked(ad().target(100.0).worth(250.0).with(costly()))),
            []
        );
        assert_eq!(
            found(tracked(ad().target(100.0).worth(150.0).with(costly()))),
            [(Kind::CostAboveTarget, 30.0)]
        );
        assert_eq!(
            found(tracked(ad().target(100.0).with(costly().value(9000.0)))),
            [(Kind::CostAboveTarget, 30.0)],
            "recorded revenue without a margin does not show a profit"
        );
    }
}
