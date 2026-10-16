mod breakdowns;
mod charts;
mod deep_breakdowns;
mod delivery;
mod recommendations;
mod translations;
pub use breakdowns::*;
pub use charts::DeviceTotal;
pub use deep_breakdowns::*;
pub use recommendations::{
    Detected, Recommendation, RecommendationState, RecommendationWithFinding,
};

use loco_rs::{Error, Result};
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement, Value};
use serde::{Deserialize, Serialize};

use super::WorkspaceStore;
use crate::monitoring::{
    access,
    causes::Cause,
    deep_check::DeepCheck,
    detection::Detection,
    metrics::{CampaignDay, Metrics, Window, WindowMetrics},
    tracking::{self, ConversionTracking},
    update::Update,
};

#[derive(Clone, Debug, PartialEq)]
pub struct EarlierUpdate {
    pub day: i64,
    pub summary: String,
    pub quiet: bool,
}

fn sql(query: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Sqlite, query, values)
}

fn update_of(row: &sea_orm::QueryResult) -> Result<Update> {
    let mut update: Update = serde_json::from_str(&row.try_get::<String>("", "data")?)?;
    update.not_useful = row.try_get::<i64>("", "not_useful")? != 0;
    Ok(update)
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct AccountSettings {
    pub judge_by_conversions: Option<bool>,
    pub weekly_email: bool,
    pub alerts: bool,
}

impl Default for AccountSettings {
    fn default() -> Self {
        Self {
            judge_by_conversions: None,
            weekly_email: true,
            alerts: true,
        }
    }
}

impl WorkspaceStore<'_> {
    fn monitoring_account(&self, id: &str) -> Vec<Value> {
        vec![self.workspace.id().into(), id.into()]
    }

    pub async fn monitoring_settings(&self, id: &str) -> Result<AccountSettings> {
        self.db.query_one_raw(sql("SELECT judge_by_conversions,weekly_email,alerts FROM monitoring_settings WHERE workspace_id=? AND ad_account_customer_id=?",self.monitoring_account(id))).await?.map_or_else(||Ok(AccountSettings::default()),|row| Ok(AccountSettings {judge_by_conversions:row.try_get::<Option<i64>>("","judge_by_conversions")?.map(|value|value!=0),weekly_email:row.try_get::<i64>("","weekly_email")?!=0,alerts:row.try_get::<i64>("","alerts")?!=0}))
    }

    pub async fn save_monitoring_settings(
        &self,
        id: &str,
        settings: &AccountSettings,
    ) -> Result<()> {
        self.save_monitoring_settings_on(self.db, id, settings)
            .await
    }

    pub(crate) async fn save_monitoring_settings_on(
        &self,
        db: &impl ConnectionTrait,
        id: &str,
        settings: &AccountSettings,
    ) -> Result<()> {
        let mut values = self.monitoring_account(id);
        values.extend([
            settings.judge_by_conversions.map(i64::from).into(),
            i64::from(settings.weekly_email).into(),
            i64::from(settings.alerts).into(),
        ]);
        db.execute_raw(sql("INSERT INTO monitoring_settings(workspace_id,ad_account_customer_id,judge_by_conversions,weekly_email,alerts) SELECT workspace_id,customer_id,?3,?4,?5 FROM ad_accounts WHERE workspace_id=?1 AND customer_id=?2 ON CONFLICT(workspace_id,ad_account_customer_id) DO UPDATE SET judge_by_conversions=excluded.judge_by_conversions,weekly_email=excluded.weekly_email,alerts=excluded.alerts",values)).await?;
        Ok(())
    }

    pub async fn monitoring_days(
        &self,
        id: &str,
        start: i64,
        end: i64,
    ) -> Result<Vec<CampaignDay>> {
        let mut values = self.monitoring_account(id);
        values.extend([start.into(), end.into()]);
        self.db.query_all_raw(sql("SELECT day,campaign_id,SUM(cost_micros) AS cost,SUM(clicks) AS clicks,SUM(impressions) AS impressions,SUM(conversions) AS conversions,SUM(conversions_value) AS conversion_value FROM campaign_days WHERE workspace_id=? AND ad_account_customer_id=? AND day BETWEEN ? AND ? GROUP BY day,campaign_id ORDER BY day,campaign_id",values)).await?.into_iter().map(|row|Ok(CampaignDay {day:row.try_get("","day")?,campaign_id:row.try_get("","campaign_id")?,metrics:Metrics {spend:row.try_get::<i64>("","cost")? as f64/1_000_000.0,clicks:row.try_get::<i64>("","clicks")? as f64,impressions:row.try_get::<i64>("","impressions")? as f64,conversions:Some(row.try_get("","conversions")?),conversion_value:Some(row.try_get("","conversion_value")?)}})).collect()
    }

    pub async fn conversion_actions(&self, id: &str) -> Result<Option<u32>> {
        Ok(self.db.query_one_raw(sql("SELECT conversion_actions FROM deep_checks WHERE workspace_id=? AND ad_account_customer_id=?",self.monitoring_account(id))).await?.map(|row|row.try_get::<Option<u32>>("","conversion_actions")).transpose()?.flatten())
    }

    pub async fn conversion_tracking(
        &self,
        id: &str,
        end: i64,
    ) -> Result<(ConversionTracking, bool)> {
        let days = self.monitoring_days(id, end - 29, end).await?;
        Ok(tracking::of_days(
            &days,
            end,
            self.conversion_actions(id).await?,
            self.monitoring_settings(id).await?.judge_by_conversions,
        ))
    }

    pub async fn window_metrics(
        &self,
        id: &str,
        end: i64,
        window: Window,
        campaign: Option<i64>,
    ) -> Result<WindowMetrics> {
        let (_, judge) = self.conversion_tracking(id, end).await?;
        let days = self
            .monitoring_days(id, end - window.days() * 2 + 1, end)
            .await?;
        Ok(crate::monitoring::metrics::window_metrics(
            &days, end, window, campaign, judge,
        ))
    }

    pub async fn updates(&self, id: &str, start: i64, end: i64) -> Result<Vec<Update>> {
        let mut values = self.monitoring_account(id);
        values.extend([start.into(), end.into()]);
        self.db.query_all_raw(sql("SELECT data,not_useful FROM updates WHERE workspace_id=? AND ad_account_customer_id=? AND day BETWEEN ? AND ? ORDER BY day DESC",values)).await?.into_iter().map(|row| update_of(&row)).collect()
    }

    pub async fn recent_updates(&self, id: &str, limit: u32) -> Result<Vec<Update>> {
        let mut values = self.monitoring_account(id);
        values.push(limit.into());
        self.db.query_all_raw(sql("SELECT data,not_useful FROM updates WHERE workspace_id=? AND ad_account_customer_id=? ORDER BY day DESC LIMIT ?",values)).await?.into_iter().map(|row| update_of(&row)).collect()
    }

    pub async fn latest_update(&self, id: &str) -> Result<Option<Update>> {
        self.db.query_one_raw(sql("SELECT data,not_useful FROM updates WHERE workspace_id=? AND ad_account_customer_id=? ORDER BY day DESC LIMIT 1",self.monitoring_account(id))).await?.map(|row| update_of(&row)).transpose()
    }

    pub async fn has_update(&self, id: &str, day: i64) -> Result<bool> {
        let mut values = self.monitoring_account(id);
        values.push(day.into());
        Ok(self
            .db
            .query_one_raw(sql(
                "SELECT 1 FROM updates WHERE workspace_id=? AND ad_account_customer_id=? AND day=?",
                values,
            ))
            .await?
            .is_some())
    }

    pub async fn earlier_summaries(
        &self,
        id: &str,
        skip: u32,
        limit: u32,
    ) -> Result<Vec<EarlierUpdate>> {
        let mut values = self.monitoring_account(id);
        values.extend([limit.into(), skip.into()]);
        self.db.query_all_raw(sql("SELECT day,json_extract(data,'$.summary') AS summary,json_extract(data,'$.detection') AS detection FROM updates WHERE workspace_id=? AND ad_account_customer_id=? ORDER BY day DESC LIMIT ? OFFSET ?",values)).await?.into_iter().map(|row|{
            let detection: Detection = serde_json::from_str(&row.try_get::<String>("","detection")?)?;
            Ok(EarlierUpdate{day:row.try_get("","day")?,summary:row.try_get("","summary")?,quiet:detection.quiet()})
        }).collect()
    }

    pub async fn campaign_update_lines(
        &self,
        id: &str,
        campaign: i64,
        limit: u32,
    ) -> Result<Vec<(i64, String, Option<Cause>)>> {
        let mut values = self.monitoring_account(id);
        values.extend([limit.into(), campaign.into()]);
        self.db.query_all_raw(sql("SELECT latest.day AS day,json_extract(line.value,'$.text') AS text,json_extract(line.value,'$.cause') AS cause FROM (SELECT day,data FROM updates WHERE workspace_id=? AND ad_account_customer_id=? ORDER BY day DESC LIMIT ?) AS latest,json_each(latest.data,'$.lines') AS line WHERE json_extract(line.value,'$.campaign_id')=? ORDER BY latest.day DESC,line.key",values)).await?.into_iter().map(|row|Ok((row.try_get("","day")?,row.try_get("","text")?,row.try_get::<Option<String>>("","cause")?.map(|cause|serde_json::from_str(&cause)).transpose()?))).collect()
    }

    pub async fn has_deep_check(&self, id: &str) -> Result<bool> {
        Ok(self
            .db
            .query_one_raw(sql(
                "SELECT 1 FROM deep_checks WHERE workspace_id=? AND ad_account_customer_id=?",
                self.monitoring_account(id),
            ))
            .await?
            .is_some())
    }

    pub async fn latest_summaries(&self) -> Result<Vec<(String, i64, String)>> {
        self.db.query_all_raw(sql("SELECT ad_account_customer_id,day,json_extract(data,'$.summary') AS summary FROM updates AS latest WHERE workspace_id=?1 AND day=(SELECT MAX(day) FROM updates WHERE workspace_id=?1 AND ad_account_customer_id=latest.ad_account_customer_id)",vec![self.workspace.id().into()])).await?.into_iter().map(|row|Ok((row.try_get("","ad_account_customer_id")?,row.try_get("","day")?,row.try_get("","summary")?))).collect()
    }

    pub async fn save_update(&self, id: &str, update: &Update) -> Result<()> {
        let mut values = self.monitoring_account(id);
        values.extend([update.day.into(), serde_json::to_string(update)?.into()]);
        self.db.execute_raw(sql("INSERT INTO updates(workspace_id,ad_account_customer_id,day,data) SELECT workspace_id,customer_id,?3,?4 FROM ad_accounts WHERE workspace_id=?1 AND customer_id=?2 ON CONFLICT(workspace_id,ad_account_customer_id,day) DO NOTHING",values)).await?;
        Ok(())
    }

    pub async fn mark_update_not_useful(&self, id: &str, day: i64) -> Result<bool> {
        let mut values = self.monitoring_account(id);
        values.push(day.into());
        Ok(self.db.execute_raw(sql("UPDATE updates SET not_useful=1 WHERE workspace_id=? AND ad_account_customer_id=? AND day=?",values)).await?.rows_affected()==1)
    }

    pub async fn latest_deep_check(&self, id: &str) -> Result<Option<DeepCheck>> {
        self.db
            .query_one_raw(sql(
                "SELECT data FROM deep_checks WHERE workspace_id=? AND ad_account_customer_id=?",
                self.monitoring_account(id),
            ))
            .await?
            .map(|row| Ok(serde_json::from_str(&row.try_get::<String>("", "data")?)?))
            .transpose()
    }

    pub async fn trial_started_at(&self) -> Result<i64> {
        self.db
            .query_one_raw(sql(
                "SELECT started_at FROM workspace_trials WHERE workspace_id=?",
                vec![self.workspace.id().into()],
            ))
            .await?
            .ok_or_else(|| Error::Message("Trial start is missing".into()))?
            .try_get("", "started_at")
            .map_err(Into::into)
    }

    pub async fn monitoring_access(&self, now: i64) -> Result<bool> {
        Ok(access::trial_open(self.trial_started_at().await?, now)
            || access::subscription_open(self.subscription().await?.as_ref(), now))
    }

    pub async fn monitoring_history_open(&self, now: i64) -> Result<bool> {
        Ok(access::history_open(
            self.trial_started_at().await?,
            self.subscription().await?.as_ref(),
            now,
        ))
    }

    pub async fn monitoring_can_select_accounts(
        &self,
        selected: &[String],
        now: i64,
    ) -> Result<bool> {
        let started = self
            .db
            .query_one_raw(sql(
                "SELECT started_at FROM workspace_trials WHERE workspace_id=? AND started=1",
                vec![self.workspace.id().into()],
            ))
            .await?;
        if started.is_some() && !self.monitoring_access(now).await? {
            return Ok(false);
        }
        let ids = self
            .ad_accounts()
            .await?
            .into_iter()
            .map(|ad| ad.customer_id)
            .chain(selected.iter().cloned())
            .collect::<std::collections::BTreeSet<_>>();
        Ok(ids.len() <= access::AD_ACCOUNT_LIMIT)
    }

    pub(crate) async fn restart_monitoring_history(&self, id: &str) -> Result<()> {
        self.db.execute_raw(sql("UPDATE account_history SET first_day=NULL,last_day=NULL WHERE workspace_id=? AND ad_account_customer_id=?",self.monitoring_account(id))).await?;
        Ok(())
    }

    pub async fn claim_monitoring_run(&self, id: &str, day: i64, now: i64) -> Result<bool> {
        let mut values = self.monitoring_account(id);
        values.extend([day.into(), now.into()]);
        Ok(self.db.execute_raw(sql("INSERT INTO monitoring_runs(workspace_id,ad_account_customer_id,day,claimed_at) SELECT workspace_id,customer_id,?3,?4 FROM ad_accounts WHERE workspace_id=?1 AND customer_id=?2 ON CONFLICT(workspace_id,ad_account_customer_id,day) DO UPDATE SET claimed_at=excluded.claimed_at WHERE completed=0 AND claimed_at<excluded.claimed_at-600",values)).await?.rows_affected()==1)
    }

    pub async fn finish_monitoring_run(
        &self,
        id: &str,
        day: i64,
        now: i64,
        completed: bool,
    ) -> Result<()> {
        let mut values = vec![i64::from(completed).into()];
        values.extend(self.monitoring_account(id));
        values.extend([day.into(), now.into()]);
        self.db.execute_raw(sql("UPDATE monitoring_runs SET completed=?1,claimed_at=CASE WHEN ?1=0 THEN 0 ELSE claimed_at END WHERE workspace_id=?2 AND ad_account_customer_id=?3 AND day=?4 AND claimed_at=?5",values)).await?;
        Ok(())
    }
}
