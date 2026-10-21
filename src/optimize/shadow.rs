use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{policy::Policy, propose::Decision};
use crate::{agent::data::AccountData, monitoring::metrics, workspace::history::day_number};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Evaluation {
    pub policy: Policy,
    pub policy_version: i64,
    pub data_day: i64,
    pub day: i64,
    pub at: i64,
    pub decisions: Vec<Decision>,
    #[serde(default)]
    pub abstention: Option<Abstention>,
    pub snapshot: Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Abstention {
    MissingCommercialTarget,
    NoSignal,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Feedback {
    Useful,
    Incorrect,
    TooRisky,
    MissingContext,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Observation {
    pub campaign_id: i64,
    pub data_day: i64,
    pub settled_days: i64,
    pub before_days: i64,
    pub before_spend: f64,
    pub before_conversions: f64,
    pub after_spend: f64,
    pub after_conversions: f64,
    pub tracking_reliable: bool,
    pub external_changes: usize,
}

impl Evaluation {
    pub fn new(
        account: &AccountData,
        policy: &Policy,
        policy_version: i64,
        day: i64,
        at: i64,
        decisions: Vec<Decision>,
    ) -> Self {
        let data_day = account.days.iter().map(|day| day.day).max().unwrap_or(0);
        Self {
            policy: policy.clone(),
            policy_version,
            data_day,
            day,
            at,
            abstention: decisions.is_empty().then(|| {
                if super::signals::target(account, policy).is_none() {
                    Abstention::MissingCommercialTarget
                } else {
                    Abstention::NoSignal
                }
            }),
            decisions,
            snapshot: json!({
                "currency": account.currency,
                "first_day": account.first_day,
                "end_day": account.end_day,
                "campaigns": account.campaigns,
                "profile": account.profile,
                "conversion_delay": account.conversion_delay,
                "tracking_reliable": super::signals::tracking_reliable(account),
                "changes": account.changes,
                "days": account.days,
                "deep_check": account.deep_check,
                "findings": account.findings,
            }),
        }
    }

    pub fn observe(&self, account: &AccountData) -> loco_rs::Result<Vec<Observation>> {
        let baseline: Vec<metrics::CampaignDay> =
            serde_json::from_value(self.snapshot["days"].clone())?;
        let (last, _) = super::signals::settled(account);
        let window = i64::from(self.policy.observation_days);
        let end = last.min(self.day + window);
        Ok(self
            .decisions
            .iter()
            .filter_map(|decision| {
                let Decision::Propose(draft) = decision else {
                    return None;
                };
                let campaign = draft.action.campaign_id();
                let before =
                    metrics::sum(&baseline, self.day - window, self.day - 1, Some(campaign));
                let after = metrics::sum(&account.days, self.day + 1, end, Some(campaign));
                let external_changes = account
                    .changes
                    .iter()
                    .filter(|change| {
                        change.campaign_id.is_none_or(|id| id == campaign)
                            && chrono::NaiveDate::parse_from_str(
                                change.changed_at.get(..10).unwrap_or_default(),
                                "%Y-%m-%d",
                            )
                            .is_ok_and(|date| (self.day - window..=end).contains(&day_number(date)))
                    })
                    .count();
                Some(Observation {
                    campaign_id: campaign,
                    data_day: account.days.iter().map(|day| day.day).max().unwrap_or(0),
                    settled_days: account
                        .days
                        .iter()
                        .filter(|row| {
                            row.campaign_id == campaign && (self.day + 1..=end).contains(&row.day)
                        })
                        .map(|row| row.day)
                        .collect::<std::collections::BTreeSet<_>>()
                        .len() as i64,
                    before_days: baseline
                        .iter()
                        .filter(|row| {
                            row.campaign_id == campaign
                                && (self.day - window..self.day).contains(&row.day)
                        })
                        .map(|row| row.day)
                        .collect::<std::collections::BTreeSet<_>>()
                        .len() as i64,
                    before_spend: before.spend,
                    before_conversions: before.conversions.unwrap_or_default(),
                    after_spend: after.spend,
                    after_conversions: after.conversions.unwrap_or_default(),
                    tracking_reliable: super::signals::tracking_reliable(account),
                    external_changes,
                })
            })
            .collect())
    }
}
