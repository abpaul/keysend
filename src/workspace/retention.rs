use loco_rs::Result;
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement, Value};

use super::WorkspaceStore;
use crate::database::begin_write;

const DAY_TABLES: [&str; 13] = [
    "campaign_days",
    "campaign_network_days",
    "campaign_search_share_days",
    "conversion_action_days",
    "conversion_lag_totals",
    "demographic_totals",
    "hour_totals",
    "keyword_quality",
    "location_totals",
    "updates",
    "alerts",
    "alert_evaluations",
    "monitoring_runs",
];

fn sql(query: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Sqlite, query, values)
}

impl WorkspaceStore<'_> {
    /// Deletes the rows of one Ad Account that are older than `oldest`, a day number. It keeps an
    /// active Finding and an open Recommendation whatever their age. `oldest_change` is the first
    /// `changed_at` that stays, as `YYYY-MM-DD HH:MM:SS`.
    pub async fn remove_history_before(
        &self,
        id: &str,
        oldest: i64,
        oldest_change: &str,
    ) -> Result<()> {
        let account = || vec![self.workspace.id().into(), id.into()];
        let before = |day: i64| {
            let mut values = account();
            values.push(day.into());
            values
        };
        let txn = begin_write(self.db).await?;
        for table in DAY_TABLES {
            txn.execute_raw(sql(
                &format!("DELETE FROM {table} WHERE workspace_id=? AND ad_account_customer_id=? AND day<?"),
                before(oldest),
            ))
            .await?;
        }
        let mut values = account();
        values.push(oldest_change.into());
        txn.execute_raw(sql(
            "DELETE FROM account_changes WHERE workspace_id=? AND ad_account_customer_id=? AND changed_at<?",
            values,
        ))
        .await?;
        txn.execute_raw(sql(
            "DELETE FROM monitoring_recommendations WHERE workspace_id=? AND ad_account_customer_id=? AND state IN ('done','not_useful') AND closed_day<?",
            before(oldest),
        ))
        .await?;
        txn.execute_raw(sql(
            "DELETE FROM monitoring_findings WHERE workspace_id=? AND ad_account_customer_id=? AND active=0 AND last_day<?",
            before(oldest),
        ))
        .await?;
        txn.commit().await?;
        Ok(())
    }
}
