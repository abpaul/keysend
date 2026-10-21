//! The Optimization policy of one Ad Account: what KeySend may change, within which limits,
//! and what needs an Approval.
//!
//! A User sets the policy; every change to it is a new version. Some rules are not settings
//! and no policy can lift them (`check`): pauses and High risk changes require Approval.
//! Shared budgets are excluded. Execution permits only budget reductions and Campaign pauses.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::{Action, ActionKind, Approval, Mode, Risk};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Policy {
    pub mode: Mode,
    /// Kinds of change that the Autopilot may run without an Approval. Only Medium risk kinds
    /// count; see `check`.
    pub automatic: BTreeSet<ActionKind>,
    /// The Campaigns that KeySend may change; none listed means every Campaign.
    pub campaigns: BTreeSet<i64>,
    /// Campaigns that KeySend never changes.
    pub protected: BTreeSet<i64>,
    /// A raise of one budget, in percent of the budget before it.
    pub max_increase_percent: u32,
    /// A cut of one budget, in percent of the budget before it.
    pub max_decrease_percent: u32,
    /// The daily budgets of the Ad Account's enabled Campaigns may add up to this amount, in
    /// micros of the Ad Account currency. None means that no raise can be approved.
    pub envelope_micros: Option<i64>,
    /// Raises of budgets that may run in one day, added up, in micros.
    pub max_daily_increase_micros: i64,
    /// Changes that may run in the Ad Account in one day.
    pub changes_per_day: u32,
    /// Days after a change of a Campaign, by anyone, before KeySend proposes another change
    /// of it.
    pub cooldown_days: u32,
    /// Days after a change by KeySend before it measures it, and in which it proposes no
    /// change in the opposite direction.
    pub observation_days: u32,
    /// Conversions that a Campaign needs in its 30 settled days before KeySend judges its cost.
    pub min_conversions: u32,
    /// The newest day of the Account history may be at most this many days before today.
    pub max_data_age_days: u32,
    /// Days between two evaluations when no new data arrived.
    pub evaluate_every_days: u32,
    /// Failed or uncertain changes in seven days that suspend the Autopilot.
    pub stop_after_failures: u32,
    /// A rise of the Ad Account's daily spend, in percent of the 7 days before, that
    /// suspends the Autopilot.
    pub stop_spend_rise_percent: u32,
    /// A target cost per conversion for Optimize, in micros; none uses the Ad Account
    /// profile.
    pub target_cost_micros: Option<i64>,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            mode: Mode::Investigate,
            automatic: BTreeSet::new(),
            campaigns: BTreeSet::new(),
            protected: BTreeSet::new(),
            max_increase_percent: 10,
            max_decrease_percent: 20,
            envelope_micros: None,
            max_daily_increase_micros: 0,
            changes_per_day: 3,
            cooldown_days: 7,
            observation_days: 14,
            min_conversions: 10,
            max_data_age_days: 2,
            evaluate_every_days: 1,
            stop_after_failures: 2,
            stop_spend_rise_percent: 50,
            target_cost_micros: None,
        }
    }
}

/// Bounds that the form of the policy accepts. Wider authority needs code.
pub const MAX_INCREASE_PERCENT: u32 = 30;
pub const MAX_DECREASE_PERCENT: u32 = 50;
pub const MAX_CHANGES_PER_DAY: u32 = 10;

impl Policy {
    pub fn eligible(&self, campaign: i64) -> bool {
        !self.protected.contains(&campaign)
            && (self.campaigns.is_empty() || self.campaigns.contains(&campaign))
    }

    /// The settings that a stored or submitted policy must satisfy.
    pub fn valid(&self) -> bool {
        (1..=MAX_INCREASE_PERCENT).contains(&self.max_increase_percent)
            && (1..=MAX_DECREASE_PERCENT).contains(&self.max_decrease_percent)
            && (1..=MAX_CHANGES_PER_DAY).contains(&self.changes_per_day)
            && (1..=90).contains(&self.cooldown_days)
            && (7..=90).contains(&self.observation_days)
            && (5..=1000).contains(&self.min_conversions)
            && (1..=7).contains(&self.max_data_age_days)
            && (1..=30).contains(&self.evaluate_every_days)
            && (1..=10).contains(&self.stop_after_failures)
            && (10..=500).contains(&self.stop_spend_rise_percent)
            && self.envelope_micros.is_none_or(|micros| micros > 0)
            && self.max_daily_increase_micros >= 0
            && self.target_cost_micros.is_none_or(|micros| micros > 0)
            && self
                .automatic
                .iter()
                .all(|kind| AUTOMATIC_KINDS.contains(kind))
    }
}

/// Only budget reductions may run automatically. Execution refuses budget increases.
pub const AUTOMATIC_KINDS: [ActionKind; 1] = [ActionKind::AdjustCampaignBudget];

/// Whether the Autopilot of an Ad Account runs, and why not.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Standing {
    Active,
    /// KeySend stopped it after a failure, a conflict or unusual spend; only the User can
    /// resume it.
    Suspended,
    /// The User switched it off. A new policy version starts it again.
    Revoked,
}

impl Standing {
    pub fn name(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Suspended => "suspended",
            Self::Revoked => "revoked",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        [Self::Active, Self::Suspended, Self::Revoked]
            .into_iter()
            .find(|standing| standing.name() == name)
    }
}

/// A stored version of the policy.
#[derive(Clone, Debug, Serialize)]
pub struct Versioned {
    pub version: i64,
    pub policy: Policy,
    pub standing: Standing,
    pub reason: String,
    pub created_at: i64,
}

/// Why the policy refuses a change. Each is shown to the User and stored in the history.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    Shadow,
    /// The Ad Account is in Investigate mode.
    Investigate,
    /// The User stopped all changes of the Workspace.
    Stopped,
    /// The Autopilot is suspended or switched off and the change needs no Approval.
    Suspended,
    NotEligible,
    StepTooLarge,
    /// The raise would take the daily budgets past the envelope, or no envelope is set.
    Envelope,
    BudgetTotalUnknown,
    Economics,
    DailyRaise,
    DailyChanges,
    /// The Campaign changed within the cooldown, by KeySend or anyone else.
    Cooldown,
    /// KeySend changed this the other way and is still observing it.
    Observing,
    /// The budget is shared by Campaigns, or Google Ads reports more than one Campaign on it.
    SharedBudget,
    /// The live value is not the value of the Proposal.
    Stale,
    Expired,
    /// The kind is not enabled for execution in this installation.
    NotLive,
    /// The Proposal is not the one that was approved.
    Fingerprint,
    /// The Google Connection no longer gives access to the Ad Account.
    Access,
    /// The policy changed after the Approval of an automatic change.
    PolicyChanged,
}

impl Refusal {
    pub fn name(self) -> &'static str {
        match self {
            Self::Shadow => "shadow",
            Self::Investigate => "investigate",
            Self::Stopped => "stopped",
            Self::Suspended => "suspended",
            Self::NotEligible => "not_eligible",
            Self::StepTooLarge => "step_too_large",
            Self::Envelope => "envelope",
            Self::BudgetTotalUnknown => "budget_total_unknown",
            Self::Economics => "economics",
            Self::DailyRaise => "daily_raise",
            Self::DailyChanges => "daily_changes",
            Self::Cooldown => "cooldown",
            Self::Observing => "observing",
            Self::SharedBudget => "shared_budget",
            Self::Stale => "stale",
            Self::Expired => "expired",
            Self::NotLive => "not_live",
            Self::Fingerprint => "fingerprint",
            Self::Access => "access",
            Self::PolicyChanged => "policy_changed",
        }
    }
}

/// What KeySend did in the Ad Account in the last days, counted from its own history.
#[derive(Clone, Debug, Default)]
pub struct Recent {
    /// Changes that ran today in the Ad Account.
    pub changes_today: u32,
    /// Raises that ran today, added up, in micros.
    pub raised_today_micros: i64,
    /// The daily budgets of the enabled Campaigns, added up, in micros, as last collected.
    /// Raises that ran today are added to it, since the collection may not include them yet.
    pub budgets_micros: i64,
    /// Days since the last change of each Campaign, by anyone.
    pub last_change_days: std::collections::BTreeMap<i64, i64>,
    /// KeySend changes in the observation window: the Campaign and whether it spent more.
    pub observing: Vec<(i64, bool)>,
}

/// The live state of a Campaign and its budget, as Google Ads returns it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Live {
    pub campaign_id: i64,
    pub status: String,
    pub budget_id: Option<i64>,
    pub budget_micros: Option<i64>,
    pub budget_shared: bool,
    /// Campaigns that use the budget, as Google Ads counts them.
    pub budget_campaigns: i64,
    pub bidding_strategy: String,
}

/// The decision of the policy on one change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Allowed(Approval),
    Refused(Refusal),
}

/// The rules that every change passes, when it is proposed and again just before it runs.
/// `standing` is the Autopilot state; a change with an Approval of the User runs while the
/// Autopilot is suspended, a change without one does not.
pub fn check(
    policy: &Policy,
    standing: Standing,
    action: &Action,
    live: Option<&Live>,
    recent: &Recent,
) -> Verdict {
    use Verdict::Refused;
    if policy.mode == Mode::Investigate {
        return Refused(Refusal::Investigate);
    }
    let campaign = action.campaign_id();
    if !policy.eligible(campaign) {
        return Refused(Refusal::NotEligible);
    }
    if let Some(live) = live {
        if live.campaign_id != campaign {
            return Refused(Refusal::Stale);
        }
    }
    let mut needs_user = action.may_spend_more() || action.risk() == Risk::High;
    if let Action::AdjustCampaignBudget {
        budget_id,
        from_micros,
        to_micros,
        ..
    } = action
    {
        if *from_micros <= 0 || *to_micros <= 0 {
            return Refused(Refusal::StepTooLarge);
        }
        let percent = (*to_micros - *from_micros) as f64 * 100.0 / *from_micros as f64;
        if percent > f64::from(policy.max_increase_percent) + 1e-9
            || -percent > f64::from(policy.max_decrease_percent) + 1e-9
        {
            return Refused(Refusal::StepTooLarge);
        }
        if let Some(live) = live {
            if live.status != "ENABLED" {
                return Refused(Refusal::Stale);
            }
            if live.budget_shared || live.budget_campaigns != 1 {
                return Refused(Refusal::SharedBudget);
            }
            if live.budget_id != Some(*budget_id) || live.budget_micros != Some(*from_micros) {
                return Refused(Refusal::Stale);
            }
        }
        if to_micros > from_micros {
            let raise = to_micros - from_micros;
            match policy.envelope_micros {
                Some(envelope)
                    if recent.budgets_micros + recent.raised_today_micros + raise <= envelope => {}
                _ => return Refused(Refusal::Envelope),
            }
            if recent.raised_today_micros + raise > policy.max_daily_increase_micros {
                return Refused(Refusal::DailyRaise);
            }
        }
    }
    if let Some(live) = live {
        let expected = match action {
            Action::PauseCampaign { .. } => Some("ENABLED"),
            Action::EnableCampaign { .. } => Some("PAUSED"),
            _ => None,
        };
        if expected.is_some_and(|status| live.status != status) {
            return Refused(Refusal::Stale);
        }
    }
    if recent.changes_today >= policy.changes_per_day {
        return Refused(Refusal::DailyChanges);
    }
    if recent
        .last_change_days
        .get(&campaign)
        .is_some_and(|days| *days < i64::from(policy.cooldown_days))
    {
        return Refused(Refusal::Cooldown);
    }
    let more = action.may_spend_more();
    if recent
        .observing
        .iter()
        .any(|(observed, spent_more)| *observed == campaign && *spent_more != more)
    {
        return Refused(Refusal::Observing);
    }
    needs_user = needs_user
        || policy.mode != Mode::Autopilot
        || standing != Standing::Active
        || !policy.automatic.contains(&action.kind());
    Verdict::Allowed(if needs_user {
        Approval::Required
    } else {
        Approval::Automatic
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn autopilot() -> Policy {
        Policy {
            mode: Mode::Autopilot,
            automatic: AUTOMATIC_KINDS.into_iter().collect(),
            envelope_micros: Some(500_000_000),
            max_daily_increase_micros: 10_000_000,
            ..Default::default()
        }
    }

    fn budget(from: i64, to: i64) -> Action {
        Action::AdjustCampaignBudget {
            campaign_id: 7,
            budget_id: 70,
            from_micros: from,
            to_micros: to,
        }
    }

    fn live(micros: i64) -> Live {
        Live {
            campaign_id: 7,
            status: "ENABLED".into(),
            budget_id: Some(70),
            budget_micros: Some(micros),
            budget_shared: false,
            budget_campaigns: 1,
            bidding_strategy: "MAXIMIZE_CONVERSIONS".into(),
        }
    }

    #[test]
    fn a_new_ad_account_changes_nothing() {
        assert_eq!(
            check(
                &Policy::default(),
                Standing::Active,
                &budget(20_000_000, 16_000_000),
                None,
                &Recent::default()
            ),
            Verdict::Refused(Refusal::Investigate)
        );
    }

    #[test]
    fn the_autopilot_cuts_a_budget_within_its_limit_without_an_approval() {
        assert_eq!(
            check(
                &autopilot(),
                Standing::Active,
                &budget(20_000_000, 16_000_000),
                Some(&live(20_000_000)),
                &Recent::default()
            ),
            Verdict::Allowed(Approval::Automatic)
        );
    }

    #[test]
    fn a_raise_a_pause_and_a_suspended_autopilot_always_wait_for_the_user() {
        let recent = Recent {
            budgets_micros: 100_000_000,
            ..Default::default()
        };
        assert_eq!(
            check(
                &autopilot(),
                Standing::Active,
                &budget(20_000_000, 22_000_000),
                Some(&live(20_000_000)),
                &recent
            ),
            Verdict::Allowed(Approval::Required)
        );
        assert_eq!(
            check(
                &autopilot(),
                Standing::Active,
                &Action::PauseCampaign { campaign_id: 7 },
                Some(&live(20_000_000)),
                &recent
            ),
            Verdict::Allowed(Approval::Required)
        );
        assert_eq!(
            check(
                &autopilot(),
                Standing::Suspended,
                &budget(20_000_000, 16_000_000),
                Some(&live(20_000_000)),
                &recent
            ),
            Verdict::Allowed(Approval::Required)
        );
    }

    #[test]
    fn a_change_past_a_limit_is_refused() {
        let policy = autopilot();
        let ok = Recent {
            budgets_micros: 100_000_000,
            ..Default::default()
        };
        let cases = [
            (
                budget(20_000_000, 15_000_000),
                live(20_000_000),
                Recent::default(),
                Refusal::StepTooLarge,
            ),
            (
                budget(20_000_000, 23_000_000),
                live(20_000_000),
                ok.clone(),
                Refusal::StepTooLarge,
            ),
            (
                budget(20_000_000, 16_000_000),
                live(18_000_000),
                Recent::default(),
                Refusal::Stale,
            ),
            (
                budget(20_000_000, 16_000_000),
                Live {
                    budget_shared: true,
                    ..live(20_000_000)
                },
                Recent::default(),
                Refusal::SharedBudget,
            ),
            (
                budget(20_000_000, 16_000_000),
                Live {
                    budget_campaigns: 2,
                    ..live(20_000_000)
                },
                Recent::default(),
                Refusal::SharedBudget,
            ),
            (
                budget(20_000_000, 22_000_000),
                live(20_000_000),
                Recent {
                    budgets_micros: 499_000_000,
                    ..Default::default()
                },
                Refusal::Envelope,
            ),
            (
                budget(200_000_000, 220_000_000),
                Live {
                    budget_micros: Some(200_000_000),
                    ..live(0)
                },
                ok.clone(),
                Refusal::DailyRaise,
            ),
            (
                budget(20_000_000, 16_000_000),
                live(20_000_000),
                Recent {
                    changes_today: 3,
                    ..Default::default()
                },
                Refusal::DailyChanges,
            ),
            (
                budget(20_000_000, 16_000_000),
                live(20_000_000),
                Recent {
                    last_change_days: [(7, 3)].into(),
                    ..Default::default()
                },
                Refusal::Cooldown,
            ),
            (
                budget(20_000_000, 22_000_000),
                live(20_000_000),
                Recent {
                    observing: vec![(7, false)],
                    ..ok.clone()
                },
                Refusal::Observing,
            ),
            (
                Action::PauseCampaign { campaign_id: 7 },
                Live {
                    status: "PAUSED".into(),
                    ..live(20_000_000)
                },
                Recent::default(),
                Refusal::Stale,
            ),
        ];
        for (action, live, recent, refusal) in cases {
            assert_eq!(
                check(&policy, Standing::Active, &action, Some(&live), &recent),
                Verdict::Refused(refusal),
                "{action:?}"
            );
        }
        let protected = Policy {
            protected: [7].into(),
            ..autopilot()
        };
        assert_eq!(
            check(
                &protected,
                Standing::Active,
                &budget(20_000_000, 16_000_000),
                None,
                &Recent::default()
            ),
            Verdict::Refused(Refusal::NotEligible)
        );
    }

    #[test]
    fn a_campaign_paused_after_the_proposal_cannot_have_its_budget_changed() {
        let paused = Live {
            status: "PAUSED".into(),
            ..live(20_000_000)
        };
        assert_eq!(
            check(
                &autopilot(),
                Standing::Active,
                &budget(20_000_000, 16_000_000),
                Some(&paused),
                &Recent::default()
            ),
            Verdict::Refused(Refusal::Stale)
        );
    }

    #[test]
    fn a_policy_cannot_let_a_high_risk_kind_run_by_itself() {
        let mut policy = autopilot();
        assert!(policy.valid());
        policy.automatic.insert(ActionKind::PauseCampaign);
        assert!(!policy.valid());
        let wide = Policy {
            max_increase_percent: 80,
            ..autopilot()
        };
        assert!(!wide.valid());
    }
}
