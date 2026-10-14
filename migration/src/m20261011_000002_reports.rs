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
            "CREATE TABLE branding (workspace_id TEXT NOT NULL, ad_account_customer_id TEXT NOT NULL, name TEXT NOT NULL, accent_colour TEXT NOT NULL, logo BLOB CHECK(logo IS NULL OR length(logo) <= 512000), PRIMARY KEY(workspace_id, ad_account_customer_id), FOREIGN KEY(workspace_id, ad_account_customer_id) REFERENCES ad_accounts(workspace_id, customer_id) ON DELETE CASCADE)",
            "CREATE TABLE reports (id TEXT PRIMARY KEY NOT NULL, workspace_id TEXT NOT NULL, ad_account_customer_id TEXT NOT NULL, reporting_start TEXT NOT NULL, reporting_end TEXT NOT NULL, comparison_start TEXT NOT NULL, comparison_end TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'Generating' CHECK(status IN ('Generating', 'Done')), created_at INTEGER NOT NULL, generation_error TEXT, generation_started_at INTEGER, email_sent_at INTEGER, email_started_at INTEGER, UNIQUE(workspace_id, id), UNIQUE(workspace_id, ad_account_customer_id, reporting_start, reporting_end, comparison_start, comparison_end), FOREIGN KEY(workspace_id, ad_account_customer_id) REFERENCES ad_accounts(workspace_id, customer_id) ON DELETE CASCADE, CHECK(reporting_start <= reporting_end), CHECK(comparison_start <= comparison_end))",
            "CREATE INDEX reports_workspace_created ON reports(workspace_id, created_at DESC)",
            "CREATE TABLE metrics_snapshots (workspace_id TEXT NOT NULL, report_id TEXT NOT NULL, metrics TEXT NOT NULL CHECK(json_valid(metrics)), PRIMARY KEY(workspace_id, report_id), FOREIGN KEY(workspace_id, report_id) REFERENCES reports(workspace_id, id) ON DELETE CASCADE)",
            "CREATE TABLE commentary (workspace_id TEXT NOT NULL, report_id TEXT NOT NULL, slide INTEGER NOT NULL CHECK(slide BETWEEN 1 AND 17), text TEXT NOT NULL, PRIMARY KEY(workspace_id, report_id, slide), FOREIGN KEY(workspace_id, report_id) REFERENCES reports(workspace_id, id) ON DELETE CASCADE)",
        ] {
            manager.get_connection().execute_unprepared(sql).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for table in ["commentary", "metrics_snapshots", "reports", "branding"] {
            manager
                .get_connection()
                .execute_unprepared(&format!("DROP TABLE {table}"))
                .await?;
        }
        Ok(())
    }
}
