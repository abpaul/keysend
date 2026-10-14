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
            "CREATE TABLE workspace_trials (workspace_id TEXT PRIMARY KEY NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE, started_at INTEGER NOT NULL, started INTEGER NOT NULL DEFAULT 0 CHECK(started IN (0,1))) WITHOUT ROWID".into(),
            "CREATE TRIGGER workspace_trial_start AFTER INSERT ON workspaces BEGIN INSERT INTO workspace_trials(workspace_id,started_at) VALUES(NEW.id,unixepoch()); END".into(),
            format!("CREATE TABLE monitoring_settings (workspace_id TEXT NOT NULL,ad_account_customer_id TEXT NOT NULL,judge_by_conversions INTEGER CHECK(judge_by_conversions IN (0,1)),weekly_email INTEGER NOT NULL DEFAULT 1 CHECK(weekly_email IN (0,1)),alerts INTEGER NOT NULL DEFAULT 1 CHECK(alerts IN (0,1)),PRIMARY KEY(workspace_id,ad_account_customer_id),{ACCOUNT}) WITHOUT ROWID"),
            format!("CREATE TABLE updates (workspace_id TEXT NOT NULL,ad_account_customer_id TEXT NOT NULL,day INTEGER NOT NULL,data TEXT NOT NULL,not_useful INTEGER NOT NULL DEFAULT 0 CHECK(not_useful IN (0,1)),PRIMARY KEY(workspace_id,ad_account_customer_id,day),{ACCOUNT}) WITHOUT ROWID"),
            format!("CREATE TABLE deep_checks (workspace_id TEXT NOT NULL,ad_account_customer_id TEXT NOT NULL,day INTEGER NOT NULL,data TEXT NOT NULL,conversion_actions INTEGER,PRIMARY KEY(workspace_id,ad_account_customer_id),{ACCOUNT}) WITHOUT ROWID"),
            format!("CREATE TABLE monitoring_findings (workspace_id TEXT NOT NULL,ad_account_customer_id TEXT NOT NULL,finding_key TEXT NOT NULL,campaign_id INTEGER,data TEXT NOT NULL,active INTEGER NOT NULL CHECK(active IN (0,1)),first_day INTEGER NOT NULL,last_day INTEGER NOT NULL,PRIMARY KEY(workspace_id,ad_account_customer_id,finding_key),{ACCOUNT}) WITHOUT ROWID"),
            format!("CREATE TABLE monitoring_recommendations (workspace_id TEXT NOT NULL,ad_account_customer_id TEXT NOT NULL,finding_key TEXT NOT NULL,data TEXT NOT NULL,state TEXT NOT NULL CHECK(state IN ('open','done','not_useful','resolved')),opened_day INTEGER NOT NULL,closed_day INTEGER,PRIMARY KEY(workspace_id,ad_account_customer_id,finding_key),FOREIGN KEY(workspace_id,ad_account_customer_id,finding_key) REFERENCES monitoring_findings(workspace_id,ad_account_customer_id,finding_key) ON DELETE CASCADE,{ACCOUNT}) WITHOUT ROWID"),
            format!("CREATE TABLE alerts (workspace_id TEXT NOT NULL,ad_account_customer_id TEXT NOT NULL,day INTEGER NOT NULL,event_key TEXT NOT NULL,data TEXT NOT NULL,body TEXT NOT NULL,created_at INTEGER NOT NULL,sent_at INTEGER,claimed_at INTEGER,cancelled_at INTEGER,PRIMARY KEY(workspace_id,ad_account_customer_id,day,event_key),{ACCOUNT}) WITHOUT ROWID"),
            "CREATE TABLE weekly_emails (workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,day INTEGER NOT NULL,data TEXT NOT NULL,body TEXT NOT NULL,created_at INTEGER NOT NULL,sent_at INTEGER,claimed_at INTEGER,cancelled_at INTEGER,PRIMARY KEY(workspace_id,day)) WITHOUT ROWID".into(),
            format!("CREATE TABLE alert_evaluations (workspace_id TEXT NOT NULL,ad_account_customer_id TEXT NOT NULL,day INTEGER NOT NULL,PRIMARY KEY(workspace_id,ad_account_customer_id,day),{ACCOUNT}) WITHOUT ROWID"),
            format!("CREATE TABLE deep_check_failures (workspace_id TEXT NOT NULL,ad_account_customer_id TEXT NOT NULL,day INTEGER NOT NULL,error TEXT NOT NULL,PRIMARY KEY(workspace_id,ad_account_customer_id),{ACCOUNT}) WITHOUT ROWID"),
            format!("CREATE TABLE monitoring_runs (workspace_id TEXT NOT NULL,ad_account_customer_id TEXT NOT NULL,day INTEGER NOT NULL,claimed_at INTEGER NOT NULL,completed INTEGER NOT NULL DEFAULT 0 CHECK(completed IN (0,1)),PRIMARY KEY(workspace_id,ad_account_customer_id,day),{ACCOUNT}) WITHOUT ROWID"),
        ] {
            manager.get_connection().execute_unprepared(&sql).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TRIGGER workspace_trial_start")
            .await?;
        for table in [
            "monitoring_runs",
            "alert_evaluations",
            "deep_check_failures",
            "weekly_emails",
            "alerts",
            "monitoring_recommendations",
            "monitoring_findings",
            "deep_checks",
            "updates",
            "monitoring_settings",
            "workspace_trials",
        ] {
            manager
                .get_connection()
                .execute_unprepared(&format!("DROP TABLE {table}"))
                .await?;
        }
        Ok(())
    }
}
