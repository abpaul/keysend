use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

async fn recreate(manager: &SchemaManager<'_>, subscription_amounts: &str) -> Result<(), DbErr> {
    let connection = manager.get_connection();
    for sql in [
        format!("CREATE TABLE billing_checkouts_new (workspace_id TEXT NOT NULL, id TEXT NOT NULL, report_id TEXT, kind TEXT NOT NULL CHECK(kind IN ('Report','Subscription')), amount INTEGER NOT NULL CHECK(amount IN (499,{subscription_amounts})), created_at INTEGER NOT NULL, paid_at INTEGER, PRIMARY KEY(workspace_id,id), FOREIGN KEY(workspace_id) REFERENCES workspaces(id) ON DELETE CASCADE, FOREIGN KEY(workspace_id,report_id) REFERENCES reports(workspace_id,id) ON DELETE CASCADE, CHECK((kind='Report' AND report_id IS NOT NULL AND amount=499) OR (kind='Subscription' AND report_id IS NULL AND amount IN ({subscription_amounts}))))"),
        "INSERT INTO billing_checkouts_new SELECT workspace_id,id,report_id,kind,amount,created_at,paid_at FROM billing_checkouts".into(),
        "CREATE TABLE report_unlocks_kept AS SELECT workspace_id,report_id,checkout_id,paid_at FROM report_unlocks".into(),
        "DROP TABLE report_unlocks".into(),
        "DROP TABLE billing_checkouts".into(),
        "ALTER TABLE billing_checkouts_new RENAME TO billing_checkouts".into(),
        "CREATE TABLE report_unlocks (workspace_id TEXT NOT NULL, report_id TEXT NOT NULL, checkout_id TEXT NOT NULL, paid_at INTEGER NOT NULL, PRIMARY KEY(workspace_id,report_id), FOREIGN KEY(workspace_id,report_id) REFERENCES reports(workspace_id,id) ON DELETE CASCADE, FOREIGN KEY(workspace_id,checkout_id) REFERENCES billing_checkouts(workspace_id,id) ON DELETE CASCADE)".into(),
        "INSERT INTO report_unlocks SELECT workspace_id,report_id,checkout_id,paid_at FROM report_unlocks_kept".into(),
        "DROP TABLE report_unlocks_kept".into(),
    ] {
        connection.execute_unprepared(&sql).await?;
    }
    Ok(())
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        Some(true)
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        recreate(manager, "3999,4900").await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DELETE FROM billing_checkouts WHERE amount=4900")
            .await?;
        recreate(manager, "3999").await
    }
}
