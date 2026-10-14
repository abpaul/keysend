use loco_rs::Result;
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement, Value};

use super::WorkspaceStore;
use crate::database::begin_write;

impl WorkspaceStore<'_> {
    pub async fn remove_ad_account(&self, customer: &str) -> Result<()> {
        self.remove_data("accounts", customer).await
    }

    pub async fn remove_google_connection(&self, id: &str) -> Result<()> {
        self.remove_data("connections", id).await
    }

    pub async fn delete_user(&self) -> Result<()> {
        self.remove_data("workspaces", self.workspace.id()).await
    }

    async fn remove_data(&self, kind: &str, id: &str) -> Result<()> {
        let txn = begin_write(self.db).await?;
        let sql = |query: &str, values: Vec<Value>| {
            Statement::from_sql_and_values(DatabaseBackend::Sqlite, query, values)
        };
        if kind == "workspaces" {
            let started = txn
                .query_one_raw(sql(
                    "SELECT u.google_subject_id AS subject,t.started_at AS started_at FROM workspaces w JOIN users u ON u.id=w.user_id JOIN workspace_trials t ON t.workspace_id=w.id WHERE w.id=? AND t.started=1",
                    vec![self.workspace.id().into()],
                ))
                .await?;
            if let Some(row) = started {
                let subject: String = row.try_get("", "subject")?;
                txn.execute_raw(sql(
                    "INSERT INTO trial_history(subject_hash,started_at) VALUES (?,?) ON CONFLICT(subject_hash) DO NOTHING",
                    vec![
                        crate::identity::subject_hash(&subject).into(),
                        row.try_get::<i64>("", "started_at")?.into(),
                    ],
                ))
                .await?;
            }
        }
        let (query, values) = match kind {
            "accounts" => (
                "DELETE FROM ad_accounts WHERE workspace_id=? AND customer_id=?",
                vec![self.workspace.id().into(), id.into()],
            ),
            "connections" => (
                "DELETE FROM google_connections WHERE workspace_id=? AND id=?",
                vec![self.workspace.id().into(), id.into()],
            ),
            _ => (
                "DELETE FROM users WHERE id=(SELECT user_id FROM workspaces WHERE id=?)",
                vec![self.workspace.id().into()],
            ),
        };
        txn.execute_raw(sql(query, values)).await?;
        txn.commit().await?;
        Ok(())
    }
}
