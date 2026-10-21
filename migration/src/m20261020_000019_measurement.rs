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
            "ALTER TABLE workspaces ADD COLUMN created_at INTEGER",
            "ALTER TABLE account_history ADD COLUMN succeeded_at INTEGER",
            "ALTER TABLE subscriptions ADD COLUMN cancel_at_period_end INTEGER NOT NULL DEFAULT 0 CHECK(cancel_at_period_end IN (0,1))",
            "ALTER TABLE subscriptions ADD COLUMN canceled_at INTEGER",
            "CREATE TABLE subscription_changes (workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE, observed_at INTEGER NOT NULL, status TEXT NOT NULL, cancel_at_period_end INTEGER NOT NULL CHECK(cancel_at_period_end IN (0,1)), PRIMARY KEY(workspace_id,observed_at,status)) WITHOUT ROWID",
            "CREATE TABLE workspace_activity (workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE, day INTEGER NOT NULL, kind TEXT NOT NULL CHECK(kind IN ('viewed','exported','feedback','connected','synced','assessed')), count INTEGER NOT NULL, PRIMARY KEY(workspace_id,day,kind)) WITHOUT ROWID",
            "CREATE INDEX workspace_activity_day_kind ON workspace_activity(day,kind)",
            "CREATE TABLE operation_days (day INTEGER NOT NULL, operation TEXT NOT NULL CHECK(operation IN ('sync','deep_check','export','google_request','ai_update','ai_review','ai_research','ai_translation')), model TEXT NOT NULL DEFAULT '', succeeded INTEGER NOT NULL, failed INTEGER NOT NULL, elapsed_ms INTEGER NOT NULL, input_tokens INTEGER, output_tokens INTEGER, PRIMARY KEY(day,operation,model)) WITHOUT ROWID",
            "CREATE TABLE business_days (day INTEGER PRIMARY KEY NOT NULL, users INTEGER NOT NULL, workspaces INTEGER NOT NULL, ad_accounts INTEGER NOT NULL, trials INTEGER NOT NULL, paying INTEGER NOT NULL, payment_due INTEGER NOT NULL, scheduled_cancellations INTEGER NOT NULL, canceled INTEGER NOT NULL, mrr_cents INTEGER NOT NULL)",
        ] {
            manager.get_connection().execute_unprepared(sql).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for sql in [
            "DROP TABLE business_days",
            "DROP TABLE operation_days",
            "DROP TABLE workspace_activity",
            "DROP TABLE subscription_changes",
            "ALTER TABLE subscriptions DROP COLUMN canceled_at",
            "ALTER TABLE subscriptions DROP COLUMN cancel_at_period_end",
            "ALTER TABLE account_history DROP COLUMN succeeded_at",
            "ALTER TABLE workspaces DROP COLUMN created_at",
        ] {
            manager.get_connection().execute_unprepared(sql).await?;
        }
        Ok(())
    }
}
