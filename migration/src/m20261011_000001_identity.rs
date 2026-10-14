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
            "CREATE TABLE users (id TEXT PRIMARY KEY NOT NULL, google_subject_id TEXT NOT NULL UNIQUE, email TEXT NOT NULL, name TEXT NOT NULL)",
            "CREATE TABLE workspaces (id TEXT PRIMARY KEY NOT NULL, user_id TEXT NOT NULL UNIQUE REFERENCES users(id) ON DELETE CASCADE)",
            "CREATE TABLE sessions (token_hash TEXT PRIMARY KEY NOT NULL, user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE, expires_at INTEGER NOT NULL)",
            "CREATE TABLE google_connections (id TEXT PRIMARY KEY NOT NULL, workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE, google_subject_id TEXT NOT NULL, email TEXT NOT NULL, encrypted_refresh_token TEXT NOT NULL, UNIQUE(workspace_id, google_subject_id), UNIQUE(workspace_id, id))",
            "CREATE TABLE ad_accounts (id TEXT PRIMARY KEY NOT NULL, workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE, google_connection_id TEXT NOT NULL, customer_id TEXT NOT NULL CHECK(length(customer_id) = 10 AND customer_id NOT GLOB '*[^0-9]*'), name TEXT NOT NULL, login_customer_id TEXT CHECK(login_customer_id IS NULL OR (length(login_customer_id) = 10 AND login_customer_id NOT GLOB '*[^0-9]*')), UNIQUE(workspace_id, customer_id), FOREIGN KEY(workspace_id, google_connection_id) REFERENCES google_connections(workspace_id, id) ON DELETE CASCADE)",
            "CREATE TABLE oauth_states (state_hash TEXT PRIMARY KEY NOT NULL, browser_hash TEXT NOT NULL, pkce_verifier TEXT NOT NULL, workspace_id TEXT REFERENCES workspaces(id) ON DELETE CASCADE, session_hash TEXT REFERENCES sessions(token_hash) ON DELETE CASCADE, expires_at INTEGER NOT NULL, CHECK((workspace_id IS NULL) = (session_hash IS NULL)))",
            "CREATE TABLE ad_account_selections (id TEXT PRIMARY KEY NOT NULL, workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE, google_connection_id TEXT NOT NULL, choices TEXT NOT NULL, expires_at INTEGER NOT NULL, FOREIGN KEY(workspace_id, google_connection_id) REFERENCES google_connections(workspace_id, id) ON DELETE CASCADE)",
        ] {
            manager.get_connection().execute_unprepared(sql).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for table in [
            "ad_account_selections",
            "oauth_states",
            "ad_accounts",
            "google_connections",
            "sessions",
            "workspaces",
            "users",
        ] {
            manager
                .get_connection()
                .execute_unprepared(&format!("DROP TABLE {table}"))
                .await?;
        }
        Ok(())
    }
}
