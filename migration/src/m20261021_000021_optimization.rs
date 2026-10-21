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
            format!("CREATE TABLE campaign_budgets (workspace_id TEXT NOT NULL, ad_account_customer_id TEXT NOT NULL, campaign_id INTEGER NOT NULL, budget_id INTEGER NOT NULL, shared INTEGER NOT NULL CHECK(shared IN (0,1)), campaigns INTEGER NOT NULL, PRIMARY KEY(workspace_id,ad_account_customer_id,campaign_id), {ACCOUNT}) WITHOUT ROWID"),
            format!("CREATE TABLE optimization_policies (workspace_id TEXT NOT NULL, ad_account_customer_id TEXT NOT NULL, version INTEGER NOT NULL CHECK(version>0), policy TEXT NOT NULL, standing TEXT NOT NULL CHECK(standing IN ('active','suspended','revoked')), reason TEXT NOT NULL DEFAULT '', created_at INTEGER NOT NULL, PRIMARY KEY(workspace_id,ad_account_customer_id,version), {ACCOUNT}) WITHOUT ROWID"),
            "CREATE TRIGGER optimization_policies_append_only BEFORE UPDATE ON optimization_policies BEGIN SELECT RAISE(ABORT,'An Optimization policy version does not change'); END".into(),
            "CREATE TABLE optimization_stops (workspace_id TEXT PRIMARY KEY REFERENCES workspaces(id) ON DELETE CASCADE, stopped_at INTEGER NOT NULL) WITHOUT ROWID".into(),
            format!("CREATE TABLE optimization_proposals (id INTEGER PRIMARY KEY, workspace_id TEXT NOT NULL, ad_account_customer_id TEXT NOT NULL, key TEXT NOT NULL, draft TEXT NOT NULL, fingerprint TEXT NOT NULL, approval TEXT NOT NULL CHECK(approval IN ('required','automatic')), state TEXT NOT NULL CHECK(state IN ('open','approved','applying','applied','failed','uncertain','conflict','rejected','expired','superseded')), approved_fingerprint TEXT, decided_at INTEGER, created_at INTEGER NOT NULL, expires_at INTEGER NOT NULL, {ACCOUNT})"),
            "CREATE UNIQUE INDEX optimization_proposals_open ON optimization_proposals(workspace_id,ad_account_customer_id,key) WHERE state IN ('open','approved','applying','uncertain')".into(),
            "CREATE INDEX optimization_proposals_account ON optimization_proposals(workspace_id,ad_account_customer_id,created_at)".into(),
            "CREATE TRIGGER optimization_proposals_fixed BEFORE UPDATE OF workspace_id,ad_account_customer_id,key,draft,fingerprint,approval,created_at,expires_at ON optimization_proposals BEGIN SELECT RAISE(ABORT,'A Proposal does not change'); END".into(),
            "CREATE TABLE optimization_executions (proposal_id INTEGER PRIMARY KEY REFERENCES optimization_proposals(id) ON DELETE CASCADE, workspace_id TEXT NOT NULL, ad_account_customer_id TEXT NOT NULL, day INTEGER NOT NULL, automatic INTEGER NOT NULL CHECK(automatic IN (0,1)), raise_micros INTEGER NOT NULL DEFAULT 0, before TEXT, after TEXT, request_id TEXT, error TEXT, verified INTEGER NOT NULL DEFAULT 0 CHECK(verified IN (0,1)), started_at INTEGER NOT NULL, finished_at INTEGER)".into(),
            "CREATE INDEX optimization_executions_account ON optimization_executions(workspace_id,ad_account_customer_id,day)".into(),
            format!("CREATE TABLE optimization_events (id INTEGER PRIMARY KEY, workspace_id TEXT NOT NULL, ad_account_customer_id TEXT NOT NULL, proposal_id INTEGER, at INTEGER NOT NULL, kind TEXT NOT NULL, detail TEXT NOT NULL, {ACCOUNT})"),
            "CREATE INDEX optimization_events_account ON optimization_events(workspace_id,ad_account_customer_id,at)".into(),
            "CREATE TRIGGER optimization_events_append_only BEFORE UPDATE ON optimization_events BEGIN SELECT RAISE(ABORT,'The optimization history is append-only'); END".into(),
            format!("CREATE TABLE optimization_evaluations (workspace_id TEXT NOT NULL, ad_account_customer_id TEXT NOT NULL, data_day INTEGER NOT NULL, evaluated_at INTEGER NOT NULL, signals INTEGER NOT NULL, proposals INTEGER NOT NULL, holds TEXT NOT NULL, PRIMARY KEY(workspace_id,ad_account_customer_id), {ACCOUNT}) WITHOUT ROWID"),
        ] {
            manager.get_connection().execute_unprepared(&sql).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for sql in [
            "DROP TABLE optimization_evaluations",
            "DROP TABLE optimization_events",
            "DROP TABLE optimization_executions",
            "DROP TABLE optimization_proposals",
            "DROP TABLE optimization_stops",
            "DROP TABLE optimization_policies",
            "DROP TABLE campaign_budgets",
        ] {
            manager.get_connection().execute_unprepared(sql).await?;
        }
        Ok(())
    }
}
