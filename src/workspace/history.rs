use chrono::{DateTime, Days, NaiveDate, Utc};
use loco_rs::{Error, Result};
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement, Value};
use serde::Serialize;

use super::WorkspaceStore;
use crate::database::begin_write;
use crate::monitoring::audit::Counts;

const EPOCH: NaiveDate = NaiveDate::from_ymd_opt(1970, 1, 1).unwrap();
const BATCH: usize = 90;

pub fn day_number(date: NaiveDate) -> i64 {
    (date - EPOCH).num_days()
}

pub fn date_of(day: i64) -> Option<NaiveDate> {
    EPOCH.checked_add_days(Days::new(u64::try_from(day).ok()?))
}

pub fn local_time(now: DateTime<Utc>, zone: &str) -> Result<DateTime<chrono_tz::Tz>> {
    zone.parse::<chrono_tz::Tz>()
        .map(|zone| now.with_timezone(&zone))
        .map_err(|_| Error::Message("Invalid Ad Account time zone".into()))
}

fn sql(query: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Sqlite, query, values)
}

#[derive(Clone, Debug, Serialize)]
pub struct Campaign {
    pub id: i64,
    pub name: String,
    pub status: String,
    pub channel_type: String,
    pub bidding_strategy: String,
    pub budget_micros: Option<i64>,
}

pub struct CampaignDay {
    pub day: NaiveDate,
    pub campaign_id: i64,
    pub device: i64,
    pub counts: Counts,
}

#[derive(Clone, Debug, Serialize)]
pub struct AccountChange {
    pub changed_at: String,
    /// The identity of the ChangeEvent, not the resource it changed.
    pub resource_name: String,
    /// Google's target resource. Older collected events may not have it.
    pub change_resource_name: Option<String>,
    pub resource_type: String,
    pub operation: String,
    pub campaign_id: Option<i64>,
    pub changed_fields: String,
    pub old_value: String,
    pub new_value: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct HistoryState {
    pub first_day: Option<NaiveDate>,
    pub last_day: Option<NaiveDate>,
    pub error: Option<String>,
    pub succeeded_at: Option<i64>,
}

impl WorkspaceStore<'_> {
    fn account(&self, id: &str) -> Vec<Value> {
        vec![self.workspace.id().into(), id.into()]
    }

    pub async fn history_state(&self, id: &str) -> Result<Option<HistoryState>> {
        self.db
            .query_one_raw(sql(
                "SELECT first_day,last_day,error,succeeded_at FROM account_history WHERE workspace_id=? AND ad_account_customer_id=?",
                self.account(id),
            ))
            .await?
            .map(|row| {
                Ok(HistoryState {
                    first_day: row.try_get::<Option<i64>>("", "first_day")?.and_then(date_of),
                    last_day: row.try_get::<Option<i64>>("", "last_day")?.and_then(date_of),
                    error: row.try_get("", "error")?,
                    succeeded_at: row.try_get("", "succeeded_at")?,
                })
            })
            .transpose()
    }

    pub async fn history_error_seen(&self, error: &str) -> Result<bool> {
        Ok(self
            .db
            .query_one_raw(sql(
                "SELECT 1 FROM account_history WHERE workspace_id=? AND error=? LIMIT 1",
                vec![self.workspace.id().into(), error.into()],
            ))
            .await?
            .is_some())
    }

    pub async fn clear_history_error(&self, connection: &str, error: &str) -> Result<()> {
        self.db.execute_raw(sql(
            "UPDATE account_history SET error=NULL WHERE workspace_id=?1 AND error=?3 AND ad_account_customer_id IN (SELECT customer_id FROM ad_accounts WHERE workspace_id=?1 AND google_connection_id=?2)",
            vec![self.workspace.id().into(), connection.into(), error.into()],
        )).await?;
        Ok(())
    }

    pub async fn history_failed(&self, id: &str, error: &str) -> Result<()> {
        let mut values = self.account(id);
        values.extend([
            crate::settings::wall_clock().timestamp().into(),
            error.into(),
        ]);
        self.db.execute_raw(sql(
            "INSERT INTO account_history(workspace_id,ad_account_customer_id,collected_at,error) SELECT workspace_id,customer_id,?3,?4 FROM ad_accounts WHERE workspace_id=?1 AND customer_id=?2 ON CONFLICT(workspace_id,ad_account_customer_id) DO UPDATE SET collected_at=excluded.collected_at,error=excluded.error",
            values,
        )).await?;
        Ok(())
    }

    pub async fn record_google_failure(&self, id: &str, detail: &str) -> Result<()> {
        let mut values = self.account(id);
        values.extend([
            crate::settings::wall_clock().timestamp().into(),
            detail.into(),
        ]);
        self.db.execute_raw(sql(
            "INSERT INTO google_failures(workspace_id,ad_account_customer_id,failed_at,detail) SELECT workspace_id,customer_id,?3,?4 FROM ad_accounts WHERE workspace_id=?1 AND customer_id=?2 ON CONFLICT(workspace_id,ad_account_customer_id) DO UPDATE SET failed_at=excluded.failed_at,detail=excluded.detail",
            values,
        )).await?;
        Ok(())
    }

    pub async fn save_history(
        &self,
        id: &str,
        from: NaiveDate,
        to: NaiveDate,
        campaigns: &[Campaign],
        days: &[CampaignDay],
        changes: &[AccountChange],
    ) -> Result<()> {
        let txn = begin_write(self.db).await?;
        if txn
            .query_one_raw(sql(
                "SELECT 1 FROM ad_accounts WHERE workspace_id=? AND customer_id=?",
                self.account(id),
            ))
            .await?
            .is_none()
        {
            return Ok(());
        }
        for campaign in campaigns {
            let mut values = self.account(id);
            values.extend([
                campaign.id.into(),
                campaign.name.clone().into(),
                campaign.status.clone().into(),
                campaign.channel_type.clone().into(),
                campaign.bidding_strategy.clone().into(),
                campaign.budget_micros.into(),
            ]);
            txn.execute_raw(sql(
                "INSERT INTO campaigns(workspace_id,ad_account_customer_id,campaign_id,name,status,channel_type,bidding_strategy,budget_micros) VALUES(?,?,?,?,?,?,?,?) ON CONFLICT(workspace_id,ad_account_customer_id,campaign_id) DO UPDATE SET name=excluded.name,status=excluded.status,channel_type=excluded.channel_type,bidding_strategy=excluded.bidding_strategy,budget_micros=excluded.budget_micros",
                values,
            )).await?;
        }
        let mut range = self.account(id);
        range.extend([day_number(from).into(), day_number(to).into()]);
        txn.execute_raw(sql(
            "DELETE FROM campaign_days WHERE workspace_id=? AND ad_account_customer_id=? AND day BETWEEN ? AND ?",
            range.clone(),
        ))
        .await?;
        for batch in days.chunks(BATCH) {
            let mut values = Vec::with_capacity(batch.len() * 10);
            for row in batch {
                values.extend(self.account(id));
                values.extend([
                    day_number(row.day).into(),
                    row.campaign_id.into(),
                    row.device.into(),
                    (row.counts.impressions.round() as i64).into(),
                    (row.counts.clicks.round() as i64).into(),
                    ((row.counts.cost * 1_000_000.0).round() as i64).into(),
                    row.counts.conversions.into(),
                    row.counts.value.into(),
                ]);
            }
            let places = vec!["(?,?,?,?,?,?,?,?,?,?)"; batch.len()].join(",");
            txn.execute_raw(sql(
                &format!("INSERT INTO campaign_days(workspace_id,ad_account_customer_id,day,campaign_id,device,impressions,clicks,cost_micros,conversions,conversions_value) VALUES{places} ON CONFLICT(workspace_id,ad_account_customer_id,day,campaign_id,device) DO UPDATE SET impressions=impressions+excluded.impressions,clicks=clicks+excluded.clicks,cost_micros=cost_micros+excluded.cost_micros,conversions=conversions+excluded.conversions,conversions_value=conversions_value+excluded.conversions_value"),
                values,
            ))
            .await?;
        }
        for change in changes {
            let mut values = self.account(id);
            values.extend([
                change.changed_at.clone().into(),
                change.resource_name.clone().into(),
                change.change_resource_name.clone().into(),
                change.resource_type.clone().into(),
                change.operation.clone().into(),
                change.campaign_id.into(),
                change.changed_fields.clone().into(),
                change.old_value.clone().into(),
                change.new_value.clone().into(),
            ]);
            txn.execute_raw(sql(
                "INSERT INTO account_changes(workspace_id,ad_account_customer_id,changed_at,resource_name,change_resource_name,resource_type,operation,campaign_id,changed_fields,old_value,new_value) VALUES(?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(workspace_id,ad_account_customer_id,changed_at,resource_name) DO UPDATE SET change_resource_name=COALESCE(account_changes.change_resource_name,excluded.change_resource_name)",
                values,
            )).await?;
        }
        range.push(crate::settings::wall_clock().timestamp().into());
        txn.execute_raw(sql(
            "INSERT INTO account_history(workspace_id,ad_account_customer_id,first_day,last_day,collected_at,succeeded_at,error) VALUES(?1,?2,?3,?4,?5,?5,NULL) ON CONFLICT(workspace_id,ad_account_customer_id) DO UPDATE SET first_day=MIN(COALESCE(first_day,excluded.first_day),excluded.first_day),last_day=MAX(COALESCE(last_day,excluded.last_day),excluded.last_day),collected_at=excluded.collected_at,succeeded_at=excluded.succeeded_at,error=NULL",
            range,
        ))
        .await?;
        txn.commit().await?;
        Ok(())
    }

    pub async fn account_changes(
        &self,
        id: &str,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Vec<AccountChange>> {
        let mut values = self.account(id);
        values.extend([
            format!("{from} 00:00:00").into(),
            format!("{to} 23:59:59.999999").into(),
        ]);
        self.db
            .query_all_raw(sql(
                "SELECT changed_at,resource_name,change_resource_name,resource_type,operation,campaign_id,changed_fields,old_value,new_value FROM account_changes WHERE workspace_id=? AND ad_account_customer_id=? AND changed_at BETWEEN ? AND ? ORDER BY changed_at",
                values,
            ))
            .await?
            .into_iter()
            .map(|row| {
                Ok(AccountChange {
                    changed_at: row.try_get("", "changed_at")?,
                    resource_name: row.try_get("", "resource_name")?,
                    change_resource_name: row.try_get("", "change_resource_name")?,
                    resource_type: row.try_get("", "resource_type")?,
                    operation: row.try_get("", "operation")?,
                    campaign_id: row.try_get("", "campaign_id")?,
                    changed_fields: row.try_get("", "changed_fields")?,
                    old_value: row.try_get("", "old_value")?,
                    new_value: row.try_get("", "new_value")?,
                })
            })
            .collect()
    }

    pub async fn ad_account_metadata(&self, id: &str) -> Result<Option<(String, String)>> {
        self.db
            .query_one_raw(sql(
                "SELECT currency,time_zone FROM ad_account_metadata WHERE workspace_id=? AND ad_account_customer_id=?",
                self.account(id),
            ))
            .await?
            .map(|row| Ok((row.try_get("", "currency")?, row.try_get("", "time_zone")?)))
            .transpose()
    }

    pub async fn campaigns(&self, id: &str) -> Result<Vec<Campaign>> {
        self.db
            .query_all_raw(sql(
                "SELECT campaign_id,name,status,channel_type,bidding_strategy,budget_micros FROM campaigns WHERE workspace_id=? AND ad_account_customer_id=? ORDER BY name,campaign_id",
                self.account(id),
            ))
            .await?
            .into_iter()
            .map(|row| {
                Ok(Campaign {
                    id: row.try_get("", "campaign_id")?,
                    name: row.try_get("", "name")?,
                    status: row.try_get("", "status")?,
                    channel_type: row.try_get("", "channel_type")?,
                    bidding_strategy: row.try_get("", "bidding_strategy")?,
                    budget_micros: row.try_get("", "budget_micros")?,
                })
            })
            .collect()
    }
}
