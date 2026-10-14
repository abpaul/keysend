use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        Some(true)
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for sql in [
            "ALTER TABLE reports ADD COLUMN metrics_source TEXT NOT NULL DEFAULT 'sample' CHECK(metrics_source IN ('sample','google'))",
            "CREATE TABLE ad_account_metadata (workspace_id TEXT NOT NULL, ad_account_customer_id TEXT NOT NULL, currency TEXT NOT NULL, time_zone TEXT NOT NULL, PRIMARY KEY(workspace_id,ad_account_customer_id), FOREIGN KEY(workspace_id,ad_account_customer_id) REFERENCES ad_accounts(workspace_id,customer_id) ON DELETE CASCADE)",
            "CREATE TABLE report_schedules (workspace_id TEXT NOT NULL, ad_account_customer_id TEXT NOT NULL, comparison TEXT NOT NULL CHECK(comparison IN ('previous','last_year')), last_reporting_end TEXT, PRIMARY KEY(workspace_id,ad_account_customer_id), FOREIGN KEY(workspace_id,ad_account_customer_id) REFERENCES ad_accounts(workspace_id,customer_id) ON DELETE CASCADE)",
            "CREATE TABLE google_ads_operations (workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE, day TEXT NOT NULL, operations INTEGER NOT NULL, PRIMARY KEY(workspace_id,day))",
        ] {
            manager.get_connection().execute_unprepared(sql).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for sql in [
            "DROP TABLE google_ads_operations",
            "DROP TABLE report_schedules",
            "DROP TABLE ad_account_metadata",
            "ALTER TABLE reports DROP COLUMN metrics_source",
        ] {
            manager.get_connection().execute_unprepared(sql).await?;
        }
        Ok(())
    }
}
