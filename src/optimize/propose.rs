//! From Signals to Proposals, or to the reason for doing nothing.
//!
//! Cost Signals can propose budget reductions and spend without conversions can propose a
//! Campaign pause. Budget increases are held until a live account-wide total is available.
//! Search-term exclusions remain recommendations. Holds retain the reason for taking no action.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{
    policy::{self, Policy, Recent, Refusal, Standing, Verdict},
    signals::{self, Caveat, Kind, Signal},
    Action, ActionKind, Draft, MatchType,
};
use crate::agent::data::AccountData;

/// The days of Account history that a Campaign needs before KeySend proposes a change.
pub const HISTORY_DAYS: i64 = 60;
/// A Proposal that nobody approved expires after this many seconds: its evidence is a week
/// old by then.
pub const EXPIRY_SECONDS: i64 = 7 * 86_400;
/// Days after a rejection in which KeySend does not propose the same change again.
pub const REJECTED_DAYS: i64 = 30;
/// Proposals open at once in one Ad Account, the largest spend first.
pub const OPEN: usize = 5;

/// The budget of a Campaign, from the Account history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Budget {
    pub budget_id: i64,
    pub shared: bool,
    pub campaigns: i64,
}

/// Why a Signal gives no change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Hold {
    Investigate,
    TrackingUnreliable,
    ConversionsNotWhole,
    StaleData,
    ShortHistory,
    NotEligible,
    Cooldown,
    Observing,
    SharedBudget,
    BudgetUnknown,
    BudgetTotalUnknown,
    /// The Campaign spends less than its budget, so a lower budget would not change its spend.
    BudgetNotBinding,
    /// No spending envelope allows a raise.
    Envelope,
    AlreadyOpen,
    RecentlyRejected,
    /// More Proposals than `OPEN` would be open; the smaller ones wait.
    Queue,
    /// The policy refused it for another reason.
    Policy,
}

impl Hold {
    pub fn name(self) -> &'static str {
        match self {
            Self::Investigate => "investigate",
            Self::TrackingUnreliable => "tracking_unreliable",
            Self::ConversionsNotWhole => "conversions_not_whole",
            Self::StaleData => "stale_data",
            Self::ShortHistory => "short_history",
            Self::NotEligible => "not_eligible",
            Self::Cooldown => "cooldown",
            Self::Observing => "observing",
            Self::SharedBudget => "shared_budget",
            Self::BudgetUnknown => "budget_unknown",
            Self::BudgetTotalUnknown => "budget_total_unknown",
            Self::BudgetNotBinding => "budget_not_binding",
            Self::Envelope => "envelope",
            Self::AlreadyOpen => "already_open",
            Self::RecentlyRejected => "recently_rejected",
            Self::Queue => "queue",
            Self::Policy => "policy",
        }
    }
}

/// One part of the explanation of a Proposal. Facts are numbers that code measured; the
/// hypothesis is what the facts might mean; the expectation gives a direction only; the risk
/// and the measure say what can go wrong and how KeySend will look at the result.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "part", rename_all = "snake_case")]
pub enum Reason {
    Fact(Signal),
    Hypothesis(Kind),
    Expected(Expected),
    Risk(RiskNote),
    Caveat(Caveat),
    Measure { days: u32 },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Expected {
    /// The daily budget falls; spend can fall by at most the difference a day.
    LessSpend {
        from: f64,
        to: f64,
    },
    MoreSpend {
        from: f64,
        to: f64,
    },
    /// The Campaign stops spending and stops recording conversions.
    NoSpend {
        daily_spend: f64,
    },
    /// The search term no longer triggers ads of the Campaign.
    TermExcluded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskNote {
    /// Conversions can fall with the spend.
    FewerConversions,
    /// More spend can bring conversions at a higher cost each.
    HigherCost,
    /// Google Ads can spend up to twice a daily budget in one day; the budget limits the
    /// average of the month.
    BudgetIsNotACeiling,
    /// The term or the Campaign may bring conversions that are recorded late or elsewhere.
    UnrecordedValue,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "decision", content = "detail", rename_all = "snake_case")]
pub enum Decision {
    Propose(Box<Draft>),
    Hold(Box<Signal>, Hold),
}

pub struct Context<'a> {
    pub policy: &'a Policy,
    pub version: i64,
    pub standing: Standing,
    pub budgets: &'a BTreeMap<i64, Budget>,
    pub recent: &'a Recent,
    /// Keys of the open Proposals of the Ad Account.
    pub open: &'a BTreeSet<String>,
    /// Keys of the Proposals that the User rejected in the last `REJECTED_DAYS`.
    pub rejected: &'a BTreeSet<String>,
    /// Today in the Ad Account's time zone.
    pub today: i64,
    pub now: i64,
    pub currency: &'a str,
}

/// The smallest step of a budget in the currency, in micros. Google Ads refuses an amount
/// that is not a multiple of it.
pub fn unit_micros(currency: &str) -> i64 {
    match currency {
        "JPY" | "KRW" | "HUF" | "CLP" | "ISK" | "TWD" | "VND" | "IDR" | "COP" | "UGX" => 1_000_000,
        _ => 10_000,
    }
}

/// A budget changed by at most `percent`, rounded towards the budget before it to the step of
/// the currency, so that rounding never takes the change past the limit.
pub fn step(from_micros: i64, percent: u32, up: bool, currency: &str) -> i64 {
    let unit = unit_micros(currency);
    let delta = from_micros as i128 * i128::from(percent) / 100;
    let delta = (delta / i128::from(unit)) as i64 * unit;
    if up {
        from_micros + delta
    } else {
        from_micros - delta
    }
}

fn key(kind: ActionKind, signal: &Signal) -> String {
    match &signal.term {
        Some(term) => format!(
            "{}:{}:{}",
            kind.name(),
            signal.campaign_id.unwrap_or_default(),
            term
        ),
        None => format!("{}:{}", kind.name(), signal.campaign_id.unwrap_or_default()),
    }
}

fn hold_of(refusal: Refusal) -> Hold {
    match refusal {
        Refusal::Investigate => Hold::Investigate,
        Refusal::NotEligible => Hold::NotEligible,
        Refusal::Cooldown => Hold::Cooldown,
        Refusal::Observing => Hold::Observing,
        Refusal::SharedBudget => Hold::SharedBudget,
        Refusal::Envelope | Refusal::DailyRaise => Hold::Envelope,
        _ => Hold::Policy,
    }
}

/// The decisions of one evaluation of an Ad Account.
pub fn decide(data: &AccountData, context: &Context) -> Vec<Decision> {
    let signals = signals::detect(data, context.policy);
    let data_day = signals
        .first()
        .map(|signal| signal.data_day)
        .unwrap_or(i64::MIN);
    let account_hold = if context.policy.mode == super::Mode::Investigate {
        Some(Hold::Investigate)
    } else if data_day < context.today - i64::from(context.policy.max_data_age_days) {
        Some(Hold::StaleData)
    } else if data
        .first_day
        .is_none_or(|first| first > data.end_day - HISTORY_DAYS)
    {
        Some(Hold::ShortHistory)
    } else {
        None
    };
    let mut decisions = Vec::new();
    let mut proposed = BTreeSet::new();
    for signal in signals {
        let hold = |hold| Decision::Hold(Box::new(signal.clone()), hold);
        if signal.kind == Kind::TrackingUnreliable {
            decisions.push(hold(Hold::TrackingUnreliable));
            continue;
        }
        if signal.kind == Kind::ConversionsNotWhole {
            decisions.push(hold(Hold::ConversionsNotWhole));
            continue;
        }
        if let Some(account_hold) = account_hold {
            decisions.push(hold(account_hold));
            continue;
        }
        let Some(campaign_id) = signal.campaign_id else {
            continue;
        };
        let (action, expected, risks) = match signal.kind {
            Kind::CostAboveTarget | Kind::BudgetLimited => {
                let up = signal.kind == Kind::BudgetLimited;
                let Some(budget) = context.budgets.get(&campaign_id) else {
                    decisions.push(hold(Hold::BudgetUnknown));
                    continue;
                };
                if budget.shared || budget.campaigns != 1 {
                    decisions.push(hold(Hold::SharedBudget));
                    continue;
                }
                if up {
                    decisions.push(hold(Hold::BudgetTotalUnknown));
                    continue;
                }
                let Some(from) = signal
                    .daily_budget
                    .map(|budget| (budget * 1_000_000.0).round() as i64)
                    .filter(|micros| *micros > 0)
                else {
                    decisions.push(hold(Hold::BudgetUnknown));
                    continue;
                };
                let percent = if up {
                    context.policy.max_increase_percent
                } else {
                    context.policy.max_decrease_percent
                };
                let to = step(from, percent, up, context.currency);
                if to == from {
                    decisions.push(hold(Hold::BudgetUnknown));
                    continue;
                }
                if !up
                    && signal
                        .daily_spend
                        .is_some_and(|spend| spend * 1_000_000.0 <= to as f64)
                {
                    decisions.push(hold(Hold::BudgetNotBinding));
                    continue;
                }
                let (from_amount, to_amount) = (from as f64 / 1e6, to as f64 / 1e6);
                (
                    Action::AdjustCampaignBudget {
                        campaign_id,
                        budget_id: budget.budget_id,
                        from_micros: from,
                        to_micros: to,
                    },
                    if up {
                        Expected::MoreSpend {
                            from: from_amount,
                            to: to_amount,
                        }
                    } else {
                        Expected::LessSpend {
                            from: from_amount,
                            to: to_amount,
                        }
                    },
                    if up {
                        vec![RiskNote::HigherCost, RiskNote::BudgetIsNotACeiling]
                    } else {
                        vec![RiskNote::FewerConversions, RiskNote::BudgetIsNotACeiling]
                    },
                )
            }
            Kind::SpendWithoutConversions => (
                Action::PauseCampaign { campaign_id },
                Expected::NoSpend {
                    daily_spend: signal.daily_spend.unwrap_or_default(),
                },
                vec![RiskNote::UnrecordedValue],
            ),
            Kind::SearchTermWithoutConversions => (
                Action::AddNegativeKeyword {
                    campaign_id,
                    text: signal.term.clone().unwrap_or_default(),
                    match_type: MatchType::Exact,
                },
                Expected::TermExcluded,
                vec![RiskNote::UnrecordedValue],
            ),
            Kind::TrackingUnreliable | Kind::ConversionsNotWhole => unreachable!("held above"),
        };
        let key = key(action.kind(), &signal);
        if context.open.contains(&key) {
            decisions.push(hold(Hold::AlreadyOpen));
            continue;
        }
        if context.rejected.contains(&key) {
            decisions.push(hold(Hold::RecentlyRejected));
            continue;
        }
        if !matches!(action, Action::AddNegativeKeyword { .. }) && !proposed.insert(campaign_id) {
            // One change of a Campaign's spend at a time; the first Signal is the strongest.
            continue;
        }
        let approval = match policy::check(
            context.policy,
            context.standing,
            &action,
            None,
            context.recent,
        ) {
            Verdict::Allowed(approval) => approval,
            Verdict::Refused(refusal) => {
                decisions.push(hold(hold_of(refusal)));
                continue;
            }
        };
        let mut reasons = vec![
            Reason::Fact(signal.clone()),
            Reason::Hypothesis(signal.kind),
            Reason::Expected(expected),
        ];
        reasons.extend(risks.into_iter().map(Reason::Risk));
        reasons.extend(signal.caveats.iter().cloned().map(Reason::Caveat));
        reasons.push(Reason::Measure {
            days: context.policy.observation_days,
        });
        decisions.push(Decision::Propose(Box::new(Draft {
            key,
            action,
            signal: signal.clone(),
            approval,
            policy_version: context.version,
            reasons,
            created_at: context.now,
            expires_at: context.now + EXPIRY_SECONDS,
        })));
    }
    order(decisions)
}

/// The largest spend first; Proposals past `OPEN` are held so that the User sees few at once.
fn order(decisions: Vec<Decision>) -> Vec<Decision> {
    let (mut proposals, holds): (Vec<_>, Vec<_>) = decisions
        .into_iter()
        .partition(|decision| matches!(decision, Decision::Propose(_)));
    proposals.sort_by(|a, b| match (a, b) {
        (Decision::Propose(a), Decision::Propose(b)) => b
            .signal
            .severity
            .cmp(&a.signal.severity)
            .then(b.signal.spend.total_cmp(&a.signal.spend)),
        _ => std::cmp::Ordering::Equal,
    });
    let mut result = Vec::new();
    for (index, decision) in proposals.into_iter().enumerate() {
        match decision {
            Decision::Propose(draft) if index >= OPEN => {
                result.push(Decision::Hold(Box::new(draft.signal), Hold::Queue))
            }
            decision => result.push(decision),
        }
    }
    result.extend(holds);
    result
}

/// The Campaign's counts over the settled days of the window that a Signal judges.
fn window(data: &AccountData, campaign_id: i64) -> crate::monitoring::metrics::Metrics {
    let (last, _) = signals::settled(data);
    crate::monitoring::metrics::sum(
        &data.days,
        last - signals::WINDOW_DAYS + 1,
        last,
        Some(campaign_id),
    )
}

/// Whether a cut of a Campaign's budget for a cost above the target would now cut conversions
/// that pay for themselves, by the profile's current economics: the rule that keeps
/// `signals::detect` from proposing such a cut, applied to a cut already proposed or approved.
pub fn cut_pays(draft: &Draft, data: &AccountData) -> bool {
    let Action::AdjustCampaignBudget {
        campaign_id,
        from_micros,
        to_micros,
        ..
    } = draft.action
    else {
        return false;
    };
    draft.signal.kind == Kind::CostAboveTarget
        && to_micros < from_micros
        && signals::pays(data, &window(data, campaign_id))
}

/// Whether an open Proposal still stands on new data: its Signal must still be there at the
/// clearing level, so that a value that moves just across the entry level does not open and
/// close a Proposal every day.
pub fn still_holds(draft: &Draft, data: &AccountData, policy: &Policy) -> bool {
    let Some(campaign_id) = draft.signal.campaign_id else {
        return false;
    };
    if !signals::tracking_reliable(data) {
        return false;
    }
    let Some(target) = signals::target(data, policy) else {
        return false;
    };
    let counts = window(data, campaign_id);
    let conversions = counts.conversions.unwrap_or_default();
    if super::stats::whole(conversions).is_none() {
        return false;
    }
    match draft.signal.kind {
        Kind::CostAboveTarget => {
            conversions > 0.0
                && counts.spend / conversions >= target * signals::CLEAR
                && !cut_pays(draft, data)
        }
        Kind::SpendWithoutConversions => conversions == 0.0,
        Kind::BudgetLimited => conversions > 0.0 && counts.spend / conversions <= target,
        Kind::SearchTermWithoutConversions => true,
        Kind::TrackingUnreliable | Kind::ConversionsNotWhole => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_budget_step_never_passes_the_limit_and_keeps_the_currency_unit() {
        assert_eq!(step(20_000_000, 20, false, "EUR"), 16_000_000);
        assert_eq!(step(20_000_000, 10, true, "EUR"), 22_000_000);
        // 10% of 33.33 is 3.333: the step rounds to 3.33.
        assert_eq!(step(33_330_000, 10, true, "EUR"), 36_660_000);
        // 10% of 1,555 yen is 155.5: the step rounds to 155.
        assert_eq!(step(1_555_000_000, 10, false, "JPY"), 1_400_000_000);
    }

    #[test]
    fn an_open_budget_cut_stops_holding_once_the_profiles_economics_show_the_campaign_paying() {
        use crate::optimize::eval::commercial::{ad, campaign, data, Account};
        let account = |economics: fn(Account) -> Account| {
            data(
                &economics(ad().target(100.0))
                    .with(campaign(1).last(31, 6200.0, 31.0))
                    .with(campaign(2).last(30, 3000.0, 40.0)),
            )
        };
        let before = account(|account| account);
        let signal = signals::detect(&before, &Policy::default())
            .into_iter()
            .find(|signal| signal.campaign_id == Some(1))
            .unwrap();
        assert_eq!(signal.kind, Kind::CostAboveTarget);
        let draft = Draft {
            key: "adjust_campaign_budget:1".into(),
            action: Action::AdjustCampaignBudget {
                campaign_id: 1,
                budget_id: 11,
                from_micros: 200_000_000,
                to_micros: 160_000_000,
            },
            signal,
            approval: crate::optimize::Approval::Required,
            policy_version: 1,
            reasons: vec![],
            created_at: 0,
            expires_at: EXPIRY_SECONDS,
        };
        assert!(still_holds(&draft, &before, &Policy::default()));
        let paying = account(|account| account.worth(250.0));
        assert!(!still_holds(&draft, &paying, &Policy::default()));
        assert!(!cut_pays(&draft, &before));
        assert!(cut_pays(&draft, &paying));
    }
}
