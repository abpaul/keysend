use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        Some(true)
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE workspaces ADD COLUMN sign_up_source TEXT CHECK(sign_up_source IS NULL OR length(sign_up_source) BETWEEN 1 AND 40)",
        ).await?;
        db.execute_unprepared(
            "ALTER TABLE oauth_states ADD COLUMN sign_up_source TEXT CHECK(sign_up_source IS NULL OR length(sign_up_source) BETWEEN 1 AND 40)",
        ).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("ALTER TABLE oauth_states DROP COLUMN sign_up_source")
            .await?;
        db.execute_unprepared("ALTER TABLE workspaces DROP COLUMN sign_up_source")
            .await?;
        Ok(())
    }
}
