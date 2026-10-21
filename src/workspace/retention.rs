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
    pub async fn remove_investigations_before(&self, oldest: i64) -> Result<()> {
        self.db
            .execute_raw(sql(
                "WITH RECURSIVE thread(root,id,created_at) AS (SELECT id,id,created_at FROM investigations WHERE workspace_id=?1 AND follows IS NULL UNION ALL SELECT t.root,i.id,i.created_at FROM investigations i JOIN thread t ON i.follows=t.id WHERE i.workspace_id=?1) DELETE FROM investigations WHERE workspace_id=?1 AND id IN (SELECT root FROM thread GROUP BY root HAVING MAX(created_at)<?2)",
                vec![self.workspace.id().into(), oldest.into()],
            ))
            .await?;
        Ok(())
    }

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
        let mut shadow_cutoff = before(oldest);
        shadow_cutoff.push(oldest_change.into());
        txn.execute_raw(sql(
            "DELETE FROM optimization_shadows WHERE workspace_id=?1 AND ad_account_customer_id=?2 AND (data_day<?3 OR json_extract(detail,'$.snapshot.first_day')<?3 OR EXISTS (SELECT 1 FROM json_each(detail,'$.snapshot.days') WHERE json_extract(value,'$.day')<?3) OR EXISTS (SELECT 1 FROM json_each(detail,'$.snapshot.changes') WHERE json_extract(value,'$.changed_at')<?4))",
            shadow_cutoff,
        ))
        .await?;
        txn.commit().await?;
        Ok(())
    }
}
