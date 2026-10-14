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
            "CREATE TABLE translations (workspace_id TEXT NOT NULL,language TEXT NOT NULL CHECK(language IN ('ro','fr','de','es','it','pt')),source_hash TEXT NOT NULL CHECK(length(source_hash)=64),ad_account_customer_id TEXT NOT NULL,text TEXT NOT NULL CHECK(length(text) BETWEEN 1 AND 2000),created_at INTEGER NOT NULL,PRIMARY KEY(workspace_id,language,source_hash,ad_account_customer_id),FOREIGN KEY(workspace_id,ad_account_customer_id) REFERENCES ad_accounts(workspace_id,customer_id) ON DELETE CASCADE) WITHOUT ROWID",
        ).await?;
        db.execute_unprepared(
            "CREATE INDEX translations_ad_account ON translations(workspace_id,ad_account_customer_id)",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE translations")
            .await?;
        Ok(())
    }
}
