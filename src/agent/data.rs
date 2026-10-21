//! Workspace-authorized snapshots for pure investigation operations.
use std::collections::BTreeMap;

use loco_rs::{Error, Result};

use super::Handle;
use crate::{
    monitoring::{
        campaign_settings::Resolved,
        causes,
        deep_check::{DeepCheck, Finding},
        detection,
        earlier::Earlier,
        metrics::{CampaignDay, Metrics},
        tracking,
    },
    workspace::{
        history::{date_of, day_number, AccountChange, Campaign},
        monitoring::{DemographicTotal, HourTotal, LocationTotal, RecommendationWithFinding},
        profile::Profile,
        WorkspaceStore,
    },
};

#[derive(Clone, Debug)]
pub struct ScopedAccount {
    pub handle: Handle,
    pub customer_id: String,
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct Scope(Vec<ScopedAccount>);

impl Scope {
    pub async fn load(store: &WorkspaceStore<'_>, requested: &[String]) -> Result<Self> {
        Ok(Self::from_accounts(&store.ad_accounts().await?, requested))
    }

    /// Stored positions define handles; an Ad Account no longer owned leaves a hole.
    pub fn from_accounts(accounts: &[crate::workspace::AdAccount], requested: &[String]) -> Self {
        Self(
            requested
                .iter()
                .take(5)
                .enumerate()
                .filter_map(|(index, id)| {
                    accounts
                        .iter()
                        .find(|account| &account.customer_id == id)
                        .map(|account| ScopedAccount {
                            handle: Handle::new(index),
                            customer_id: account.customer_id.clone(),
                            name: account.name.clone(),
                        })
                })
                .collect(),
        )
    }

    pub fn resolve(&self, handle: Handle) -> Option<&ScopedAccount> {
        self.0.iter().find(|account| account.handle == handle)
    }

    pub fn accounts(&self) -> &[ScopedAccount] {
        &self.0
    }
}

pub struct DeviceData {
    pub device: i64,
    pub current: Metrics,
    pub previous: Option<Metrics>,
}

pub struct AccountData {
    pub customer_id: String,
    pub name: String,
    pub currency: String,
    /// Last completed day, supplied by the caller in the Ad Account's time zone.
    pub end_day: i64,
    pub first_day: Option<i64>,
    pub days: Vec<CampaignDay>,
    pub campaigns: Vec<Campaign>,
    pub changes: Vec<AccountChange>,
    pub rules: Resolved,
    pub profile: Profile,
    pub deep_check: Option<DeepCheck>,
    pub findings: Vec<Finding>,
    pub finding_history: Vec<(Finding, bool)>,
    pub earlier: Vec<Earlier>,
    pub recommendations: Vec<RecommendationWithFinding>,
    pub action_weeks: Vec<(i64, String, f64)>,
    pub devices: Vec<DeviceData>,
    pub locations: Vec<LocationTotal>,
    pub hours: Vec<HourTotal>,
    pub demographics: Vec<(String, DemographicTotal)>,
    pub conversion_delay: Option<(f64, f64)>,
    /// Daily detection and its gathered Cause candidates keep their own seven-day Window.
    pub detection: detection::Detection,
}

pub async fn load(
    store: &WorkspaceStore<'_>,
    customer_id: &str,
    end_day: i64,
) -> Result<AccountData> {
    let account = store
        .ad_accounts()
        .await?
        .into_iter()
        .find(|a| a.customer_id == customer_id)
        .ok_or_else(|| Error::Message("Unknown Ad Account".into()))?;
    let (currency, _) = store
        .ad_account_metadata(customer_id)
        .await?
        .ok_or_else(|| Error::Message("Ad Account currency is missing".into()))?;
    let from = date_of(end_day - 730).ok_or_else(|| Error::Message("Invalid day".into()))?;
    let to = date_of(end_day).ok_or_else(|| Error::Message("Invalid day".into()))?;
    let days = store
        .monitoring_days(customer_id, end_day - 730, end_day)
        .await?;
    let campaigns = store.campaigns(customer_id).await?;
    let changes = store.account_changes(customer_id, from, to).await?;
    let settings = store.monitoring_settings(customer_id).await?;
    let (state, _) = tracking::of_days(
        &days,
        end_day,
        store.conversion_actions(customer_id).await?,
        settings.judge_by_conversions,
    );
    let rules = store.campaign_rules(customer_id, state, &settings).await?;
    let first_day = store
        .history_state(customer_id)
        .await?
        .and_then(|s| s.first_day)
        .map(day_number);
    let mut detected = detection::detect(
        &days,
        &campaigns,
        &changes,
        end_day,
        first_day.unwrap_or(end_day),
        &rules,
        &currency,
    );
    detected.causes = causes::candidates(
        &detected,
        &causes::gather(store, customer_id, &days, &changes, &campaigns, &detected).await?,
    );
    let current_devices = store
        .device_metrics(customer_id, end_day - 29, end_day, None)
        .await?;
    let previous_devices = if first_day.is_some_and(|first| first <= end_day - 59) {
        Some(
            store
                .device_metrics(customer_id, end_day - 59, end_day - 30, None)
                .await?,
        )
    } else {
        None
    };
    let device_ids = current_devices
        .keys()
        .chain(previous_devices.iter().flat_map(|p| p.keys()))
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    let devices = device_ids
        .into_iter()
        .map(|device| DeviceData {
            device,
            current: current_devices.get(&device).cloned().unwrap_or_default(),
            previous: previous_devices
                .as_ref()
                .map(|p| p.get(&device).cloned().unwrap_or_default()),
        })
        .collect();
    Ok(AccountData {
        customer_id: account.customer_id,
        name: account.name,
        currency,
        end_day,
        first_day,
        days,
        campaigns,
        changes,
        rules,
        detection: detected,
        profile: store.profile(customer_id).await?,
        deep_check: store.latest_deep_check(customer_id).await?,
        findings: store.monitoring_findings(customer_id).await?,
        finding_history: store.monitoring_finding_history(customer_id).await?,
        earlier: store.recommendation_history(customer_id).await?,
        recommendations: store.recommendations_with_findings(customer_id).await?,
        action_weeks: store
            .conversion_action_weeks(customer_id, end_day - 55, end_day)
            .await?,
        devices,
        locations: store.location_totals(customer_id).await?,
        hours: store.hour_totals(customer_id).await?,
        demographics: store.demographic_totals(customer_id).await?,
        conversion_delay: store.conversion_delay(customer_id).await?,
    })
}

/// Load only the Ad Accounts whose handles were authorized for this investigation.
pub async fn load_scope(
    store: &WorkspaceStore<'_>,
    scope: &Scope,
    end_day: i64,
) -> Result<BTreeMap<Handle, AccountData>> {
    let mut snapshots = BTreeMap::new();
    for account in scope.accounts() {
        snapshots.insert(
            account.handle,
            load(store, &account.customer_id, end_day).await?,
        );
    }
    Ok(snapshots)
}

#[cfg(test)]
impl Scope {
    pub(super) fn synthetic(accounts: Vec<ScopedAccount>) -> Self {
        Self(accounts)
    }

    pub(super) fn example() -> Self {
        Self(vec![ScopedAccount {
            handle: Handle::new(0),
            customer_id: crate::monitoring::example::AD.into(),
            name: "Harbour Dental".into(),
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::AdAccount;

    #[test]
    fn a_users_remaining_ad_account_keeps_its_stored_handle() {
        let scope = Scope::from_accounts(
            &[AdAccount {
                customer_id: "remaining".into(),
                name: "Remaining Ad Account".into(),
                google_connection_id: "connection".into(),
                login_customer_id: None,
            }],
            &["removed".into(), "remaining".into()],
        );
        assert!(scope.resolve(Handle::new(0)).is_none());
        assert_eq!(
            scope.resolve(Handle::new(1)).unwrap().customer_id,
            "remaining"
        );
    }
}
