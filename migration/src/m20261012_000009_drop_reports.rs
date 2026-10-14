use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        Some(true)
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        for sql in [
            "CREATE TABLE billing_checkouts_new (workspace_id TEXT NOT NULL, id TEXT NOT NULL, amount INTEGER NOT NULL CHECK(amount IN (3999,4900)), created_at INTEGER NOT NULL, paid_at INTEGER, PRIMARY KEY(workspace_id,id), FOREIGN KEY(workspace_id) REFERENCES workspaces(id) ON DELETE CASCADE)",
            "INSERT INTO billing_checkouts_new SELECT workspace_id,id,amount,created_at,paid_at FROM billing_checkouts WHERE kind='Subscription'",
            "DROP TABLE report_unlocks",
            "DROP TABLE report_feedback",
            "DROP TABLE recommendations",
            "DROP TABLE commentary",
            "DROP TABLE metrics_snapshots",
            "DROP TABLE billing_checkouts",
            "ALTER TABLE billing_checkouts_new RENAME TO billing_checkouts",
            "DROP TABLE reports",
            "DROP TABLE campaign_exclusions",
            "DROP TABLE report_schedules",
        ] {
            connection.execute_unprepared(sql).await?;
        }
        connection
            .execute_unprepared("INSERT INTO workspace_trials(workspace_id,started_at) SELECT id,unixepoch() FROM workspaces WHERE id NOT IN (SELECT workspace_id FROM workspace_trials)")
            .await?;
        if manager.has_table("sqlt_loco_queue").await? {
            connection
                .execute_unprepared(
                    "DELETE FROM sqlt_loco_queue WHERE name IN ('ReportWorker','AnalysisWorker')",
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Migration(
            "The Report tables are deleted and cannot be restored".into(),
        ))
    }
}
