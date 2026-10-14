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
            "CREATE TABLE stripe_customers (workspace_id TEXT PRIMARY KEY NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE, stripe_customer_id TEXT NOT NULL UNIQUE)",
            "CREATE TABLE billing_checkouts (workspace_id TEXT NOT NULL, id TEXT NOT NULL, report_id TEXT, kind TEXT NOT NULL CHECK(kind IN ('Report','Subscription')), amount INTEGER NOT NULL CHECK(amount IN (499,3999)), created_at INTEGER NOT NULL, paid_at INTEGER, PRIMARY KEY(workspace_id,id), FOREIGN KEY(workspace_id) REFERENCES workspaces(id) ON DELETE CASCADE, FOREIGN KEY(workspace_id,report_id) REFERENCES reports(workspace_id,id) ON DELETE CASCADE, CHECK((kind='Report' AND report_id IS NOT NULL AND amount=499) OR (kind='Subscription' AND report_id IS NULL AND amount=3999)))",
            "CREATE TABLE report_unlocks (workspace_id TEXT NOT NULL, report_id TEXT NOT NULL, checkout_id TEXT NOT NULL, paid_at INTEGER NOT NULL, PRIMARY KEY(workspace_id,report_id), FOREIGN KEY(workspace_id,report_id) REFERENCES reports(workspace_id,id) ON DELETE CASCADE, FOREIGN KEY(workspace_id,checkout_id) REFERENCES billing_checkouts(workspace_id,id) ON DELETE CASCADE)",
            "CREATE TABLE subscriptions (workspace_id TEXT PRIMARY KEY NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE, stripe_subscription_id TEXT NOT NULL, created_at INTEGER NOT NULL, status TEXT NOT NULL, period_start INTEGER NOT NULL, period_end INTEGER NOT NULL)",
            "CREATE TABLE stripe_events (workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE, id TEXT NOT NULL, PRIMARY KEY(workspace_id,id))",
        ] {
            manager.get_connection().execute_unprepared(sql).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for table in [
            "stripe_events",
            "subscriptions",
            "report_unlocks",
            "billing_checkouts",
            "stripe_customers",
        ] {
            manager
                .get_connection()
                .execute_unprepared(&format!("DROP TABLE {table}"))
                .await?;
        }
        Ok(())
    }
}
