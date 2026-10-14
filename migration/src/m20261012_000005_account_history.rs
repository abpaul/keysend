use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const ACCOUNT: &str = "FOREIGN KEY(workspace_id,ad_account_customer_id) REFERENCES ad_accounts(workspace_id,customer_id) ON DELETE CASCADE";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        Some(true)
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for sql in [
            format!("CREATE TABLE campaigns (workspace_id TEXT NOT NULL, ad_account_customer_id TEXT NOT NULL, campaign_id INTEGER NOT NULL, name TEXT NOT NULL, status TEXT NOT NULL, channel_type TEXT NOT NULL, bidding_strategy TEXT NOT NULL, budget_micros INTEGER, PRIMARY KEY(workspace_id,ad_account_customer_id,campaign_id), {ACCOUNT}) WITHOUT ROWID"),
            format!("CREATE TABLE campaign_days (workspace_id TEXT NOT NULL, ad_account_customer_id TEXT NOT NULL, day INTEGER NOT NULL, campaign_id INTEGER NOT NULL, device INTEGER NOT NULL, impressions INTEGER NOT NULL, clicks INTEGER NOT NULL, cost_micros INTEGER NOT NULL, conversions REAL NOT NULL, conversions_value REAL NOT NULL, PRIMARY KEY(workspace_id,ad_account_customer_id,day,campaign_id,device), {ACCOUNT}) WITHOUT ROWID"),
            format!("CREATE TABLE account_changes (workspace_id TEXT NOT NULL, ad_account_customer_id TEXT NOT NULL, changed_at TEXT NOT NULL, resource_name TEXT NOT NULL, resource_type TEXT NOT NULL, operation TEXT NOT NULL, campaign_id INTEGER, changed_fields TEXT NOT NULL, old_value TEXT NOT NULL, new_value TEXT NOT NULL, PRIMARY KEY(workspace_id,ad_account_customer_id,changed_at,resource_name), {ACCOUNT}) WITHOUT ROWID"),
            format!("CREATE TABLE account_history (workspace_id TEXT NOT NULL, ad_account_customer_id TEXT NOT NULL, first_day INTEGER, last_day INTEGER, collected_at INTEGER, error TEXT, PRIMARY KEY(workspace_id,ad_account_customer_id), {ACCOUNT}) WITHOUT ROWID"),
            format!("CREATE TABLE google_failures (workspace_id TEXT NOT NULL, ad_account_customer_id TEXT NOT NULL, failed_at INTEGER NOT NULL, detail TEXT NOT NULL, PRIMARY KEY(workspace_id,ad_account_customer_id), {ACCOUNT}) WITHOUT ROWID"),
        ] {
            manager.get_connection().execute_unprepared(&sql).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for table in [
            "google_failures",
            "account_history",
            "account_changes",
            "campaign_days",
            "campaigns",
        ] {
            manager
                .get_connection()
                .execute_unprepared(&format!("DROP TABLE {table}"))
                .await?;
        }
        Ok(())
    }
}
