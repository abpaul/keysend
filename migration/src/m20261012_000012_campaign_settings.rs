use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        Some(true)
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.get_connection().execute_unprepared(
            "CREATE TABLE campaign_settings (workspace_id TEXT NOT NULL,ad_account_customer_id TEXT NOT NULL,campaign_id INTEGER NOT NULL,target_cost_micros INTEGER CHECK(target_cost_micros IS NULL OR target_cost_micros>0),note TEXT CHECK(note IS NULL OR length(note) BETWEEN 1 AND 500),alerts INTEGER CHECK(alerts IN (0,1)),judge_by_conversions INTEGER CHECK(judge_by_conversions IN (0,1)),PRIMARY KEY(workspace_id,ad_account_customer_id,campaign_id),FOREIGN KEY(workspace_id,ad_account_customer_id) REFERENCES ad_accounts(workspace_id,customer_id) ON DELETE CASCADE) WITHOUT ROWID",
        ).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE campaign_settings")
            .await?;
        Ok(())
    }
}
