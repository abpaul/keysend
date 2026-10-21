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
            "ALTER TABLE account_changes ADD COLUMN change_resource_name TEXT",
            "CREATE TABLE optimization_shadows (id INTEGER PRIMARY KEY, workspace_id TEXT NOT NULL, ad_account_customer_id TEXT NOT NULL, policy_version INTEGER NOT NULL, data_day INTEGER NOT NULL, at INTEGER NOT NULL, detail TEXT NOT NULL, UNIQUE(workspace_id,ad_account_customer_id,policy_version,data_day), FOREIGN KEY(workspace_id,ad_account_customer_id) REFERENCES ad_accounts(workspace_id,customer_id) ON DELETE CASCADE, FOREIGN KEY(workspace_id,ad_account_customer_id,policy_version) REFERENCES optimization_policies(workspace_id,ad_account_customer_id,version) ON DELETE CASCADE)",
            "CREATE INDEX optimization_shadows_account ON optimization_shadows(workspace_id,ad_account_customer_id,at)",
            "CREATE TRIGGER optimization_shadows_fixed BEFORE UPDATE ON optimization_shadows BEGIN SELECT RAISE(ABORT,'A Shadow evaluation does not change'); END",
            "CREATE TABLE optimization_shadow_events (id INTEGER PRIMARY KEY, shadow_id INTEGER NOT NULL REFERENCES optimization_shadows(id) ON DELETE CASCADE, at INTEGER NOT NULL, kind TEXT NOT NULL CHECK(kind IN ('feedback','observation')), detail TEXT NOT NULL)",
            "CREATE INDEX optimization_shadow_events_parent ON optimization_shadow_events(shadow_id,at)",
            "CREATE TRIGGER optimization_shadow_events_fixed BEFORE UPDATE ON optimization_shadow_events BEGIN SELECT RAISE(ABORT,'Shadow evidence is append-only'); END",
        ] {
            manager.get_connection().execute_unprepared(sql).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("ALTER TABLE account_changes DROP COLUMN change_resource_name")
            .await?;
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE optimization_shadow_events")
            .await?;
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE optimization_shadows")
            .await?;
        Ok(())
    }
}
