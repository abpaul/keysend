//! Optimize: Proposals for changes to an Ad Account, their Approval, their execution through
//! Google Ads, and what followed.
//!
//! The pipeline is: Account history → Signals (`signals`) → Proposals or a reason for no
//! action (`propose`) → the Optimization policy (`policy`) → Approval by the User, or the
//! Autopilot policy → execution with a check of the live state (`execute`) → read back →
//! Outcome (`outcome`). Code does every step. No model writes a Proposal, a value or a
//! decision, and no model can call Google Ads.
//!
//! A change is one [`Action`] of a closed list with typed arguments. An Approval binds the
//! [`Proposal::fingerprint`] of the exact change that the User saw; execution refuses a
//! Proposal whose fingerprint differs from its Approval.

pub mod ads;
pub mod cycle;
#[cfg(test)]
mod eval;
pub mod execute;
pub mod outcome;
pub mod policy;
pub mod propose;
pub mod shadow;
pub mod signals;
pub mod stats;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// How far KeySend may go for one Ad Account. Investigate is the default: no Proposal is
/// made and nothing is changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Investigate,
    /// Record decisions and observations without creating an executable Proposal.
    Shadow,
    /// KeySend proposes changes; each one runs only after the User approves it.
    Recommend,
    /// Changes that the policy authorizes run without an Approval; every other change
    /// waits for one.
    Autopilot,
}

/// The kinds of change that exist. A model cannot add one: the list is this enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    AdjustCampaignBudget,
    PauseCampaign,
    EnableCampaign,
    AddNegativeKeyword,
    PauseKeyword,
    AdjustTargetCpa,
    AdjustTargetRoas,
}

impl ActionKind {
    pub const ALL: [Self; 7] = [
        Self::AdjustCampaignBudget,
        Self::PauseCampaign,
        Self::EnableCampaign,
        Self::AddNegativeKeyword,
        Self::PauseKeyword,
        Self::AdjustTargetCpa,
        Self::AdjustTargetRoas,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::AdjustCampaignBudget => "adjust_campaign_budget",
            Self::PauseCampaign => "pause_campaign",
            Self::EnableCampaign => "enable_campaign",
            Self::AddNegativeKeyword => "add_negative_keyword",
            Self::PauseKeyword => "pause_keyword",
            Self::AdjustTargetCpa => "adjust_target_cpa",
            Self::AdjustTargetRoas => "adjust_target_roas",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchType {
    Exact,
    Phrase,
}

/// One change, with the value that KeySend read before it and the value it proposes. The
/// Google Ads resources are named by their IDs within the Ad Account of the Proposal; code
/// builds the resource names from the Ad Account it resolved, never from these values alone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    AdjustCampaignBudget {
        campaign_id: i64,
        budget_id: i64,
        from_micros: i64,
        to_micros: i64,
    },
    PauseCampaign {
        campaign_id: i64,
    },
    EnableCampaign {
        campaign_id: i64,
    },
    AddNegativeKeyword {
        campaign_id: i64,
        text: String,
        match_type: MatchType,
    },
    PauseKeyword {
        campaign_id: i64,
        ad_group_id: i64,
        criterion_id: i64,
    },
    AdjustTargetCpa {
        campaign_id: i64,
        from_micros: i64,
        to_micros: i64,
    },
    /// A target return on ad spend in thousandths: 4.5 is 4500.
    AdjustTargetRoas {
        campaign_id: i64,
        from_milli: i64,
        to_milli: i64,
    },
}

/// What a change can cost the advertiser when it is wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    Medium,
    High,
}

impl Action {
    pub fn kind(&self) -> ActionKind {
        match self {
            Self::AdjustCampaignBudget { .. } => ActionKind::AdjustCampaignBudget,
            Self::PauseCampaign { .. } => ActionKind::PauseCampaign,
            Self::EnableCampaign { .. } => ActionKind::EnableCampaign,
            Self::AddNegativeKeyword { .. } => ActionKind::AddNegativeKeyword,
            Self::PauseKeyword { .. } => ActionKind::PauseKeyword,
            Self::AdjustTargetCpa { .. } => ActionKind::AdjustTargetCpa,
            Self::AdjustTargetRoas { .. } => ActionKind::AdjustTargetRoas,
        }
    }

    pub fn campaign_id(&self) -> i64 {
        match self {
            Self::AdjustCampaignBudget { campaign_id, .. }
            | Self::PauseCampaign { campaign_id }
            | Self::EnableCampaign { campaign_id }
            | Self::AddNegativeKeyword { campaign_id, .. }
            | Self::PauseKeyword { campaign_id, .. }
            | Self::AdjustTargetCpa { campaign_id, .. }
            | Self::AdjustTargetRoas { campaign_id, .. } => *campaign_id,
        }
    }

    /// A budget raise, a Campaign that starts again and a looser target can spend more.
    pub fn may_spend_more(&self) -> bool {
        match self {
            Self::AdjustCampaignBudget {
                from_micros,
                to_micros,
                ..
            } => to_micros > from_micros,
            Self::EnableCampaign { .. } => true,
            Self::AdjustTargetCpa {
                from_micros,
                to_micros,
                ..
            } => to_micros > from_micros,
            Self::AdjustTargetRoas {
                from_milli,
                to_milli,
                ..
            } => to_milli < from_milli,
            Self::PauseCampaign { .. }
            | Self::AddNegativeKeyword { .. }
            | Self::PauseKeyword { .. } => false,
        }
    }

    pub fn risk(&self) -> Risk {
        match self {
            Self::AdjustCampaignBudget { .. } if !self.may_spend_more() => Risk::Medium,
            Self::AddNegativeKeyword { .. } | Self::PauseKeyword { .. } => Risk::Medium,
            _ => Risk::High,
        }
    }

    /// The change that restores the value before this one, when one exists. The restoring
    /// change is a new Proposal and needs its own Approval.
    pub fn reverse(&self) -> Option<Self> {
        Some(match self {
            Self::AdjustCampaignBudget {
                campaign_id,
                budget_id,
                from_micros,
                to_micros,
            } => Self::AdjustCampaignBudget {
                campaign_id: *campaign_id,
                budget_id: *budget_id,
                from_micros: *to_micros,
                to_micros: *from_micros,
            },
            Self::PauseCampaign { campaign_id } => Self::EnableCampaign {
                campaign_id: *campaign_id,
            },
            Self::EnableCampaign { campaign_id } => Self::PauseCampaign {
                campaign_id: *campaign_id,
            },
            Self::AdjustTargetCpa {
                campaign_id,
                from_micros,
                to_micros,
            } => Self::AdjustTargetCpa {
                campaign_id: *campaign_id,
                from_micros: *to_micros,
                to_micros: *from_micros,
            },
            Self::AdjustTargetRoas {
                campaign_id,
                from_milli,
                to_milli,
            } => Self::AdjustTargetRoas {
                campaign_id: *campaign_id,
                from_milli: *to_milli,
                to_milli: *from_milli,
            },
            Self::AddNegativeKeyword { .. } | Self::PauseKeyword { .. } => return None,
        })
    }
}

/// Whether a Proposal waits for the User or may run under the Autopilot policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Approval {
    Required,
    Automatic,
}

/// The life of a Proposal. Only these transitions exist: Open → Approved → Applying →
/// Applied, Failed, Uncertain or Conflict; Uncertain → Applied, Failed or Conflict after a
/// read of the live state; Open → Rejected, Expired or Superseded. An Applied, Failed,
/// Conflict, Rejected, Expired or Superseded Proposal never changes again.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalState {
    Open,
    Approved,
    Applying,
    Applied,
    Failed,
    /// Google Ads did not confirm the change and KeySend could not yet read the result.
    Uncertain,
    /// The live value was not the one the Proposal was made for, or it changed in between.
    Conflict,
    Rejected,
    Expired,
    Superseded,
}

impl ProposalState {
    pub const ALL: [Self; 10] = [
        Self::Open,
        Self::Approved,
        Self::Applying,
        Self::Applied,
        Self::Failed,
        Self::Uncertain,
        Self::Conflict,
        Self::Rejected,
        Self::Expired,
        Self::Superseded,
    ];

    pub fn finished(self) -> bool {
        matches!(
            self,
            Self::Applied
                | Self::Failed
                | Self::Conflict
                | Self::Rejected
                | Self::Expired
                | Self::Superseded
        )
    }

    pub fn may_become(self, next: Self) -> bool {
        matches!(
            (self, next),
            (
                Self::Open,
                Self::Approved | Self::Rejected | Self::Expired | Self::Superseded
            ) | (
                Self::Approved,
                Self::Applying | Self::Expired | Self::Failed
            ) | (
                Self::Applying,
                Self::Applied | Self::Failed | Self::Uncertain | Self::Conflict
            ) | (
                Self::Uncertain,
                Self::Applied | Self::Failed | Self::Conflict
            )
        )
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Approved => "approved",
            Self::Applying => "applying",
            Self::Applied => "applied",
            Self::Failed => "failed",
            Self::Uncertain => "uncertain",
            Self::Conflict => "conflict",
            Self::Rejected => "rejected",
            Self::Expired => "expired",
            Self::Superseded => "superseded",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|state| state.name() == name)
    }
}

/// A Proposal before the store gives it an ID: the change, why code made it, and the
/// conditions under which it may run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Draft {
    /// One open Proposal at most has a key in an Ad Account: the Campaign and the kind.
    pub key: String,
    pub action: Action,
    pub signal: signals::Signal,
    pub approval: Approval,
    /// The version of the Optimization policy that the Proposal was made under.
    pub policy_version: i64,
    /// What the User reads, as facts, a hypothesis, the expected direction, the risk and
    /// how to measure: each part is a code-written fact, rendered in the User's language.
    pub reasons: Vec<propose::Reason>,
    pub created_at: i64,
    pub expires_at: i64,
}

impl Draft {
    /// The digest of the exact change: the Ad Account, the change, the policy version and the
    /// expiry. An Approval stores it; execution refuses a Proposal with another one.
    pub fn fingerprint(&self, customer_id: &str) -> String {
        let canonical = serde_json::json!({
            "ad_account": customer_id,
            "action": self.action,
            "policy_version": self.policy_version,
            "expires_at": self.expires_at,
        });
        Sha256::digest(canonical.to_string().as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

/// A stored Proposal.
#[derive(Clone, Debug, Serialize)]
pub struct Proposal {
    pub id: i64,
    pub ad_account: String,
    pub draft: Draft,
    pub fingerprint: String,
    pub state: ProposalState,
    pub decided_at: Option<i64>,
    /// The fingerprint that the Approval bound, when there is one.
    pub approved: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_budget_raise_is_high_risk_and_a_budget_cut_is_medium_risk() {
        let raise = Action::AdjustCampaignBudget {
            campaign_id: 1,
            budget_id: 2,
            from_micros: 20_000_000,
            to_micros: 22_000_000,
        };
        assert_eq!(raise.risk(), Risk::High);
        assert!(raise.may_spend_more());
        let cut = raise.reverse().unwrap();
        assert_eq!(cut.risk(), Risk::Medium);
        assert!(!cut.may_spend_more());
        assert_eq!(
            Action::PauseCampaign { campaign_id: 1 }.risk(),
            Risk::High,
            "a pause stops results"
        );
    }

    #[test]
    fn an_action_outside_the_list_or_with_an_unknown_field_is_refused() {
        assert!(serde_json::from_value::<Action>(
            serde_json::json!({"kind":"remove_campaign","campaign_id":1})
        )
        .is_err());
        assert!(serde_json::from_value::<Action>(
            serde_json::json!({"kind":"pause_campaign","campaign_id":1,"customer_id":"999"})
        )
        .is_err());
    }

    #[test]
    fn only_the_listed_proposal_transitions_exist() {
        use ProposalState::*;
        assert!(Open.may_become(Approved));
        assert!(Approved.may_become(Applying));
        assert!(Uncertain.may_become(Applied));
        assert!(
            !Applied.may_become(Applying),
            "an applied change runs twice"
        );
        assert!(
            !Rejected.may_become(Approved),
            "a rejection is approved later"
        );
        assert!(
            !Open.may_become(Applying),
            "a change runs without an Approval"
        );
        for state in ProposalState::ALL {
            if state.finished() {
                assert!(ProposalState::ALL
                    .iter()
                    .all(|next| !state.may_become(*next)));
            }
        }
    }
}
