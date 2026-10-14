use sea_orm_migration::prelude::*;

const SENTENCE: &str = " Results cannot be judged.";

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        Some(true)
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(&format!(
                "UPDATE updates SET data=json_set(data,'$.summary',substr(json_extract(data,'$.summary'),1,length(json_extract(data,'$.summary'))-{length})) WHERE json_extract(data,'$.summary') LIKE '%{SENTENCE}'",
                length = SENTENCE.len()
            ))
            .await?;
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }
}
