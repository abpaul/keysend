use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const REPORT: &str =
    "FOREIGN KEY(workspace_id,report_id) REFERENCES reports(workspace_id,id) ON DELETE CASCADE";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        Some(true)
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for sql in [
            "ALTER TABLE reports ADD COLUMN kind_override TEXT CHECK(kind_override IN ('performance','traffic'))".to_owned(),
            "ALTER TABLE reports ADD COLUMN verdict TEXT NOT NULL DEFAULT ''".to_owned(),
            "ALTER TABLE reports ADD COLUMN analysis_started_at INTEGER".to_owned(),
            "ALTER TABLE reports ADD COLUMN question TEXT NOT NULL DEFAULT 'review' CHECK(question IN ('review','waste','change','growth'))".to_owned(),
            "ALTER TABLE reports ADD COLUMN excluded_campaigns TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(excluded_campaigns))".to_owned(),
            "CREATE TABLE campaign_exclusions (workspace_id TEXT NOT NULL, ad_account_customer_id TEXT NOT NULL, campaign_id INTEGER NOT NULL, PRIMARY KEY(workspace_id,ad_account_customer_id,campaign_id), FOREIGN KEY(workspace_id,ad_account_customer_id) REFERENCES ad_accounts(workspace_id,customer_id) ON DELETE CASCADE) WITHOUT ROWID".to_owned(),
            "ALTER TABLE commentary ADD COLUMN origin TEXT NOT NULL DEFAULT 'edited' CHECK(origin IN ('ai','edited'))".to_owned(),
            "ALTER TABLE reports ADD COLUMN verdict_origin TEXT NOT NULL DEFAULT 'none' CHECK(verdict_origin IN ('none','ai','edited'))".to_owned(),
            format!("CREATE TABLE recommendations (workspace_id TEXT NOT NULL, report_id TEXT NOT NULL, id TEXT NOT NULL, position INTEGER NOT NULL, finding_key TEXT, action TEXT NOT NULL, reason TEXT NOT NULL, evidence TEXT NOT NULL, origin TEXT NOT NULL CHECK(origin IN ('ai','edited','user')), PRIMARY KEY(workspace_id,report_id,id), {REPORT}) WITHOUT ROWID"),
            format!("CREATE TABLE report_feedback (workspace_id TEXT NOT NULL, report_id TEXT NOT NULL, target TEXT NOT NULL CHECK(target IN ('finding','recommendation')), key TEXT NOT NULL, text TEXT NOT NULL, reason TEXT NOT NULL, created_at INTEGER NOT NULL, PRIMARY KEY(workspace_id,report_id,target,key), {REPORT}) WITHOUT ROWID"),
            "CREATE TABLE ad_account_profiles (workspace_id TEXT NOT NULL, ad_account_customer_id TEXT NOT NULL, goal TEXT NOT NULL DEFAULT '' CHECK(goal IN ('','leads','sales','calls','visits')), target_cost_micros INTEGER, note TEXT NOT NULL DEFAULT '', PRIMARY KEY(workspace_id,ad_account_customer_id), FOREIGN KEY(workspace_id,ad_account_customer_id) REFERENCES ad_accounts(workspace_id,customer_id) ON DELETE CASCADE) WITHOUT ROWID".to_owned(),
        ] {
            manager.get_connection().execute_unprepared(&sql).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for sql in [
            "DROP TABLE ad_account_profiles",
            "DROP TABLE report_feedback",
            "DROP TABLE recommendations",
            "ALTER TABLE commentary DROP COLUMN origin",
            "DROP TABLE campaign_exclusions",
            "ALTER TABLE reports DROP COLUMN excluded_campaigns",
            "ALTER TABLE reports DROP COLUMN question",
            "ALTER TABLE reports DROP COLUMN analysis_started_at",
            "ALTER TABLE reports DROP COLUMN verdict_origin",
            "ALTER TABLE reports DROP COLUMN verdict",
            "ALTER TABLE reports DROP COLUMN kind_override",
        ] {
            manager.get_connection().execute_unprepared(sql).await?;
        }
        Ok(())
    }
}
