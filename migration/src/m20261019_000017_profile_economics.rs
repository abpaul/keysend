use sea_orm_migration::prelude::*;

const COLUMNS: [(&str, &str); 4] = [
    ("margin_basis_points", "BETWEEN 1 AND 10000"),
    ("customer_value_micros", "> 0"),
    ("close_rate_basis_points", "BETWEEN 1 AND 10000"),
    ("target_return_hundredths", "> 0"),
];

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        Some(true)
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for (column, check) in COLUMNS {
            manager
                .get_connection()
                .execute_unprepared(&format!(
                    "ALTER TABLE ad_account_profiles ADD COLUMN {column} INTEGER CHECK({column} {check})"
                ))
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for (column, _) in COLUMNS {
            manager
                .get_connection()
                .execute_unprepared(&format!(
                    "ALTER TABLE ad_account_profiles DROP COLUMN {column}"
                ))
                .await?;
        }
        Ok(())
    }
}
