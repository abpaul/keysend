//! The narrow door to Google Ads for a change: read the live state of one Campaign, and send
//! one typed change, first as a validation only and then for real. `google_ads::mutate`
//! implements it over REST; [`Simulated`] implements it in memory with the behaviors that
//! execution must survive, for the tests and the evaluation.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::{policy::Live, Action};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AdsError {
    /// Google Ads refused the change; nothing changed. The code is Google's error code.
    Refused { code: String },
    /// The Google Connection lost access: a revoked token, or a User without the right role.
    Access,
    /// A quota or rate limit; nothing changed.
    Quota,
    /// KeySend sent nothing: no connection, or its own daily limit of Google Ads operations.
    Unavailable,
    /// The request was sent and no answer came back. The change may have been applied.
    Unknown,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Applied {
    /// The `request-id` that Google Ads returns, for its support and for the history.
    pub request_id: Option<String>,
}

#[async_trait]
pub trait Ads: Send + Sync {
    async fn campaign(&self, campaign_id: i64) -> Result<Option<Live>, AdsError>;
    async fn apply(&self, action: &Action, validate_only: bool) -> Result<Applied, AdsError>;
}

/// The value that a change sets, and the value before it, in the terms of [`Live`].
pub fn sets(action: &Action, live: &Live) -> Option<bool> {
    if !same_resource(action, live) {
        return Some(false);
    }
    match action {
        Action::AdjustCampaignBudget { to_micros, .. } => {
            Some(live.budget_micros == Some(*to_micros))
        }
        Action::PauseCampaign { .. } => Some(live.status == "PAUSED"),
        Action::EnableCampaign { .. } => Some(live.status == "ENABLED"),
        _ => None,
    }
}

pub fn before(action: &Action, live: &Live) -> Option<bool> {
    if !same_resource(action, live) {
        return Some(false);
    }
    match action {
        Action::AdjustCampaignBudget { from_micros, .. } => {
            Some(live.budget_micros == Some(*from_micros))
        }
        Action::PauseCampaign { .. } => Some(live.status == "ENABLED"),
        Action::EnableCampaign { .. } => Some(live.status == "PAUSED"),
        _ => None,
    }
}

fn same_resource(action: &Action, live: &Live) -> bool {
    live.campaign_id == action.campaign_id()
        && match action {
            Action::AdjustCampaignBudget { budget_id, .. } => {
                live.budget_id == Some(*budget_id)
                    && !live.budget_shared
                    && live.budget_campaigns == 1
            }
            _ => true,
        }
}

/// A behavior of the next request to [`Simulated`].
#[derive(Clone, Debug)]
pub enum Fault {
    /// The next validation or change is refused with this code.
    Refuse(&'static str),
    /// The next change is applied, and its answer is lost.
    LostAnswer,
    /// The next change is not applied and its answer is lost.
    Timeout,
    /// The Google Connection is revoked from now on.
    Revoke,
    /// Someone changes the budget to this amount after KeySend reads it and before it writes.
    ExternalBudget(i64),
    /// The next read fails with no answer.
    ReadUnknown,
}

/// An in-memory Google Ads with Campaigns, budgets that may be shared, and the faults above.
/// Like Google Ads, it sets the absolute value of a change whatever the value before it: there
/// is no compare-and-set. A change made by someone else between KeySend's read and its write
/// is overwritten; execution reads again after validation to narrow that window.
pub struct Simulated {
    state: Mutex<SimulatedState>,
}

#[derive(Default)]
struct SimulatedState {
    campaigns: BTreeMap<i64, Live>,
    faults: VecDeque<Fault>,
    revoked: bool,
    requests: Vec<(Action, bool)>,
    next_request: u64,
}

impl Simulated {
    pub fn new(campaigns: Vec<Live>) -> Self {
        Self {
            state: Mutex::new(SimulatedState {
                campaigns: campaigns
                    .into_iter()
                    .map(|live| (live.campaign_id, live))
                    .collect(),
                ..Default::default()
            }),
        }
    }

    pub fn fault(&self, fault: Fault) {
        self.state
            .lock()
            .expect("simulated state")
            .faults
            .push_back(fault);
    }

    /// The changes that were sent, with whether each was a validation only.
    pub fn requests(&self) -> Vec<(Action, bool)> {
        self.state.lock().expect("simulated state").requests.clone()
    }

    /// The changes that were sent for real.
    pub fn sent(&self) -> usize {
        self.requests()
            .iter()
            .filter(|(_, validate)| !validate)
            .count()
    }

    pub fn live(&self, campaign_id: i64) -> Option<Live> {
        self.state
            .lock()
            .expect("simulated state")
            .campaigns
            .get(&campaign_id)
            .cloned()
    }

    pub fn set(&self, live: Live) {
        self.state
            .lock()
            .expect("simulated state")
            .campaigns
            .insert(live.campaign_id, live);
    }
}

#[async_trait]
impl Ads for Simulated {
    async fn campaign(&self, campaign_id: i64) -> Result<Option<Live>, AdsError> {
        let mut state = self.state.lock().expect("simulated state");
        if state.revoked {
            return Err(AdsError::Access);
        }
        if matches!(state.faults.front(), Some(Fault::ReadUnknown)) {
            state.faults.pop_front();
            return Err(AdsError::Unknown);
        }
        Ok(state.campaigns.get(&campaign_id).cloned())
    }

    async fn apply(&self, action: &Action, validate_only: bool) -> Result<Applied, AdsError> {
        let mut state = self.state.lock().expect("simulated state");
        state.requests.push((action.clone(), validate_only));
        if matches!(state.faults.front(), Some(Fault::Revoke)) {
            state.faults.pop_front();
            state.revoked = true;
        }
        if state.revoked {
            return Err(AdsError::Access);
        }
        if !validate_only {
            if let Some(Fault::ExternalBudget(micros)) = state.faults.front().cloned() {
                state.faults.pop_front();
                if let Some(live) = state.campaigns.get_mut(&action.campaign_id()) {
                    live.budget_micros = Some(micros);
                }
            }
        }
        let fault = match state.faults.front() {
            Some(Fault::Refuse(_)) => state.faults.pop_front(),
            Some(Fault::LostAnswer | Fault::Timeout) if !validate_only => state.faults.pop_front(),
            _ => None,
        };
        if let Some(Fault::Refuse(code)) = fault {
            return Err(AdsError::Refused { code: code.into() });
        }
        let Some(live) = state.campaigns.get(&action.campaign_id()).cloned() else {
            return Err(AdsError::Refused {
                code: "RESOURCE_NOT_FOUND".into(),
            });
        };
        let updated = match action {
            Action::AdjustCampaignBudget {
                budget_id,
                to_micros,
                ..
            } => {
                if live.budget_id != Some(*budget_id) {
                    return Err(AdsError::Refused {
                        code: "RESOURCE_NOT_FOUND".into(),
                    });
                }
                if *to_micros % 10_000 != 0 || *to_micros < 10_000 {
                    return Err(AdsError::Refused {
                        code: "NON_MULTIPLE_OF_MINIMUM_CURRENCY_UNIT".into(),
                    });
                }
                Live {
                    budget_micros: Some(*to_micros),
                    ..live.clone()
                }
            }
            Action::PauseCampaign { .. } => Live {
                status: "PAUSED".into(),
                ..live.clone()
            },
            Action::EnableCampaign { .. } => Live {
                status: "ENABLED".into(),
                ..live.clone()
            },
            _ => {
                return Err(AdsError::Refused {
                    code: "UNSUPPORTED".into(),
                })
            }
        };
        if validate_only {
            return Ok(Applied::default());
        }
        if matches!(fault, Some(Fault::Timeout)) {
            return Err(AdsError::Unknown);
        }
        // A shared budget changes for every Campaign that uses it.
        if let Action::AdjustCampaignBudget {
            budget_id,
            to_micros,
            ..
        } = action
        {
            for other in state.campaigns.values_mut() {
                if other.budget_id == Some(*budget_id) {
                    other.budget_micros = Some(*to_micros);
                }
            }
        }
        state.campaigns.insert(updated.campaign_id, updated);
        state.next_request += 1;
        if matches!(fault, Some(Fault::LostAnswer)) {
            return Err(AdsError::Unknown);
        }
        Ok(Applied {
            request_id: Some(format!("simulated-{}", state.next_request)),
        })
    }
}

#[cfg(test)]
mod safety_tests {
    use super::*;

    #[test]
    fn an_ad_accounts_readback_must_identify_the_exact_unshared_budget() {
        let action = Action::AdjustCampaignBudget {
            campaign_id: 7,
            budget_id: 70,
            from_micros: 80_000_000,
            to_micros: 64_000_000,
        };
        let live = Live {
            campaign_id: 7,
            budget_id: Some(70),
            budget_micros: Some(64_000_000),
            status: "ENABLED".into(),
            budget_shared: false,
            budget_campaigns: 1,
            bidding_strategy: "MAXIMIZE_CONVERSIONS".into(),
        };
        assert_eq!(sets(&action, &live), Some(true));
        for changed in [
            Live {
                campaign_id: 8,
                ..live.clone()
            },
            Live {
                budget_id: Some(80),
                ..live.clone()
            },
            Live {
                budget_shared: true,
                ..live.clone()
            },
            Live {
                budget_campaigns: 0,
                ..live.clone()
            },
            Live {
                budget_campaigns: 2,
                ..live.clone()
            },
        ] {
            assert_eq!(sets(&action, &changed), Some(false));
            assert_eq!(
                before(
                    &action,
                    &Live {
                        budget_micros: Some(80_000_000),
                        ..changed
                    }
                ),
                Some(false)
            );
        }
        assert_eq!(
            sets(
                &Action::PauseCampaign { campaign_id: 8 },
                &Live {
                    status: "PAUSED".into(),
                    ..live.clone()
                }
            ),
            Some(false)
        );
        assert_eq!(
            before(&Action::PauseCampaign { campaign_id: 8 }, &live),
            Some(false)
        );
    }
}
