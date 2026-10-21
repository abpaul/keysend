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
            "CREATE TABLE admin_challenges(token_hash TEXT PRIMARY KEY NOT NULL, email TEXT NOT NULL, code_mac TEXT NOT NULL, expires_at INTEGER NOT NULL, attempts INTEGER NOT NULL DEFAULT 0)",
            "CREATE INDEX admin_challenges_email ON admin_challenges(email)",
            "CREATE TABLE admin_sessions(token_hash TEXT PRIMARY KEY NOT NULL, email TEXT NOT NULL, created_at INTEGER NOT NULL, expires_at INTEGER NOT NULL)",
            "CREATE TABLE admin_throttle(key TEXT PRIMARY KEY NOT NULL, window_start INTEGER NOT NULL, count INTEGER NOT NULL)",
        ] { manager.get_connection().execute_unprepared(sql).await?; }
        Ok(())
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for table in ["admin_challenges", "admin_sessions", "admin_throttle"] {
            manager
                .get_connection()
                .execute_unprepared(&format!("DROP TABLE {table}"))
                .await?;
        }
        Ok(())
    }
}
