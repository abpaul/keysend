use loco_rs::Result;
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

use super::WorkspaceStore;

const DAILY_HISTORY_OPERATIONS: u32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Daily,
    Later,
}

impl WorkspaceStore<'_> {
    pub async fn claim_google_operation(&self, limit: u32, operation: Operation) -> Result<bool> {
        let limit = match operation {
            Operation::Daily => limit,
            Operation::Later => {
                let accounts: i64 = self
                    .db
                    .query_one_raw(Statement::from_string(
                        DatabaseBackend::Sqlite,
                        "SELECT COUNT(*) AS accounts FROM ad_accounts",
                    ))
                    .await?
                    .map_or(Ok(0), |row| row.try_get("", "accounts"))?;
                limit.saturating_sub(
                    u32::try_from(accounts)
                        .unwrap_or(u32::MAX)
                        .saturating_mul(DAILY_HISTORY_OPERATIONS),
                )
            }
        };
        let day = crate::settings::wall_clock()
            .with_timezone(&chrono_tz::America::Los_Angeles)
            .date_naive()
            .to_string();
        Ok(self.db.execute_raw(Statement::from_sql_and_values(DatabaseBackend::Sqlite,
            "INSERT INTO google_ads_operations(workspace_id,day,operations) SELECT ?,?,1 WHERE (SELECT COALESCE(SUM(operations),0) FROM google_ads_operations WHERE day=?)<? ON CONFLICT(workspace_id,day) DO UPDATE SET operations=operations+1 WHERE (SELECT COALESCE(SUM(operations),0) FROM google_ads_operations WHERE day=?)<?",
            vec![self.workspace.id().into(), day.clone().into(), day.clone().into(), limit.into(), day.into(), limit.into()])).await?.rows_affected() == 1)
    }

    pub async fn save_ad_account_metadata(
        &self,
        id: &str,
        currency: &str,
        zone: &str,
    ) -> Result<()> {
        self.db.execute_raw(Statement::from_sql_and_values(DatabaseBackend::Sqlite,
            "INSERT INTO ad_account_metadata(workspace_id,ad_account_customer_id,currency,time_zone) SELECT workspace_id,customer_id,?,? FROM ad_accounts WHERE workspace_id=? AND customer_id=? ON CONFLICT(workspace_id,ad_account_customer_id) DO UPDATE SET currency=excluded.currency,time_zone=excluded.time_zone",
            vec![currency.into(), zone.into(), self.workspace.id().into(), id.into()])).await?;
        Ok(())
    }
}
