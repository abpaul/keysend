use std::collections::BTreeMap;

use loco_rs::Result;
use sea_orm::{ConnectionTrait, DatabaseBackend, QueryResult, Statement, Value};

use super::{monitoring::AccountSettings, WorkspaceStore};
use crate::monitoring::{
    campaign_settings::{AccountDefaults, CampaignSettings, Choice, Resolved},
    tracking::{self, ConversionTracking},
};

fn sql(query: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Sqlite, query, values)
}

fn settings_of(row: &QueryResult) -> Result<(i64, CampaignSettings)> {
    let flag = |column: &str| {
        Ok::<_, loco_rs::Error>(
            row.try_get::<Option<i64>>("", column)?
                .map(|flag| flag != 0),
        )
    };
    Ok((
        row.try_get("", "campaign_id")?,
        CampaignSettings {
            target_cost: row
                .try_get::<Option<i64>>("", "target_cost_micros")?
                .map(|micros| micros as f64 / 1_000_000.0),
            note: row.try_get("", "note")?,
            alerts: Choice::from_flag(flag("alerts")?),
            judge_by_conversions: Choice::from_flag(flag("judge_by_conversions")?),
        },
    ))
}

impl WorkspaceStore<'_> {
    pub async fn campaign_settings(
        &self,
        ad_account: &str,
    ) -> Result<BTreeMap<i64, CampaignSettings>> {
        self.db
            .query_all_raw(sql(
                "SELECT campaign_id,target_cost_micros,note,alerts,judge_by_conversions FROM campaign_settings WHERE workspace_id=? AND ad_account_customer_id=?",
                vec![self.workspace.id().into(), ad_account.into()],
            ))
            .await?
            .iter()
            .map(settings_of)
            .collect()
    }

    pub async fn save_campaign_settings(
        &self,
        ad_account: &str,
        campaign: i64,
        settings: &CampaignSettings,
    ) -> Result<bool> {
        if !settings.valid() {
            return Ok(false);
        }
        Ok(self
            .db
            .execute_raw(sql(
                "INSERT INTO campaign_settings(workspace_id,ad_account_customer_id,campaign_id,target_cost_micros,note,alerts,judge_by_conversions) SELECT workspace_id,customer_id,?3,?4,?5,?6,?7 FROM ad_accounts WHERE workspace_id=?1 AND customer_id=?2 ON CONFLICT(workspace_id,ad_account_customer_id,campaign_id) DO UPDATE SET target_cost_micros=excluded.target_cost_micros,note=excluded.note,alerts=excluded.alerts,judge_by_conversions=excluded.judge_by_conversions",
                vec![
                    self.workspace.id().into(),
                    ad_account.into(),
                    campaign.into(),
                    settings
                        .target_cost
                        .map(|cost| (cost * 1_000_000.0).round() as i64)
                        .into(),
                    settings.note.clone().into(),
                    settings.alerts.flag().map(i64::from).into(),
                    settings.judge_by_conversions.flag().map(i64::from).into(),
                ],
            ))
            .await?
            .rows_affected()
            == 1)
    }

    pub async fn campaign_rules(
        &self,
        ad_account: &str,
        state: ConversionTracking,
        monitoring: &AccountSettings,
    ) -> Result<Resolved> {
        let rows = self
            .db
            .query_all_raw(sql(
                "SELECT p.target_cost_micros AS account_target,c.campaign_id,c.target_cost_micros,c.note,c.alerts,c.judge_by_conversions FROM ad_accounts a LEFT JOIN ad_account_profiles p ON p.workspace_id=a.workspace_id AND p.ad_account_customer_id=a.customer_id LEFT JOIN campaign_settings c ON c.workspace_id=a.workspace_id AND c.ad_account_customer_id=a.customer_id WHERE a.workspace_id=? AND a.customer_id=?",
                vec![self.workspace.id().into(), ad_account.into()],
            ))
            .await?;
        let mut target_cost = None;
        let mut own = BTreeMap::new();
        for row in &rows {
            target_cost = row
                .try_get::<Option<i64>>("", "account_target")?
                .map(|micros| micros as f64 / 1_000_000.0);
            if row.try_get::<Option<i64>>("", "campaign_id")?.is_some() {
                own.extend([settings_of(row)?]);
            }
        }
        Ok(Resolved::new(
            AccountDefaults {
                state,
                judged: tracking::judge(state, monitoring.judge_by_conversions),
                target_cost,
                alerts: monitoring.alerts,
            },
            own,
        ))
    }

    pub async fn resolved_settings(&self, ad_account: &str, end: i64) -> Result<Resolved> {
        let days = self.monitoring_days(ad_account, end - 29, end).await?;
        let monitoring = self.monitoring_settings(ad_account).await?;
        let (state, _) = tracking::of_days(
            &days,
            end,
            self.conversion_actions(ad_account).await?,
            monitoring.judge_by_conversions,
        );
        self.campaign_rules(ad_account, state, &monitoring).await
    }
}
