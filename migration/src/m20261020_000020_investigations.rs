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
            "CREATE TABLE investigations (id INTEGER PRIMARY KEY, workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE, follows INTEGER REFERENCES investigations(id) ON DELETE CASCADE, question TEXT NOT NULL CHECK(length(question)<=500), accounts TEXT NOT NULL, period TEXT NOT NULL CHECK(period IN ('last_7_days','last_30_days')), state TEXT NOT NULL CHECK(state IN ('created','gathering','answering','completed','failed','cancelled')), failure TEXT CHECK(failure IN ('daily_limit','budget','model','no_access','interrupted')), report TEXT, model_calls INTEGER NOT NULL DEFAULT 0, input_tokens INTEGER NOT NULL DEFAULT 0, output_tokens INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL, started_at INTEGER, finished_at INTEGER)",
            "CREATE INDEX investigations_workspace_created ON investigations(workspace_id,created_at)",
            "CREATE TABLE investigation_steps (investigation_id INTEGER NOT NULL REFERENCES investigations(id) ON DELETE CASCADE, ordinal INTEGER NOT NULL, round INTEGER NOT NULL, operation TEXT NOT NULL, outcome TEXT NOT NULL CHECK(outcome IN ('answered','refused','repeated')), evidence TEXT NOT NULL, elapsed_ms INTEGER NOT NULL, PRIMARY KEY(investigation_id,ordinal)) WITHOUT ROWID",
            "ALTER TABLE operation_days RENAME TO operation_days_previous",
            "CREATE TABLE operation_days (day INTEGER NOT NULL, operation TEXT NOT NULL CHECK(operation IN ('sync','deep_check','export','google_request','ai_update','ai_review','ai_research','ai_translation','ai_investigation')), model TEXT NOT NULL DEFAULT '', succeeded INTEGER NOT NULL, failed INTEGER NOT NULL, elapsed_ms INTEGER NOT NULL, input_tokens INTEGER, output_tokens INTEGER, PRIMARY KEY(day,operation,model)) WITHOUT ROWID",
            "INSERT INTO operation_days SELECT * FROM operation_days_previous",
            "DROP TABLE operation_days_previous",
        ] {
            manager.get_connection().execute_unprepared(sql).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for sql in [
            "DROP TABLE investigation_steps",
            "DROP TABLE investigations",
            "ALTER TABLE operation_days RENAME TO operation_days_previous",
            "CREATE TABLE operation_days (day INTEGER NOT NULL, operation TEXT NOT NULL CHECK(operation IN ('sync','deep_check','export','google_request','ai_update','ai_review','ai_research','ai_translation')), model TEXT NOT NULL DEFAULT '', succeeded INTEGER NOT NULL, failed INTEGER NOT NULL, elapsed_ms INTEGER NOT NULL, input_tokens INTEGER, output_tokens INTEGER, PRIMARY KEY(day,operation,model)) WITHOUT ROWID",
            "INSERT INTO operation_days SELECT * FROM operation_days_previous WHERE operation!='ai_investigation'",
            "DROP TABLE operation_days_previous",
        ] {
            manager.get_connection().execute_unprepared(sql).await?;
        }
        Ok(())
    }
}
