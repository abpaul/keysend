use loco_rs::Result;
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement, Value};
use serde::Serialize;

use super::WorkspaceStore;

fn sql(query: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Sqlite, query, values)
}

#[derive(Clone, Default, Serialize)]
pub struct Profile {
    pub goal: String,
    pub target_cost: Option<f64>,
    pub note: String,
}

impl Profile {
    pub fn valid(&self) -> bool {
        matches!(
            self.goal.as_str(),
            "" | "leads" | "sales" | "calls" | "visits"
        ) && self.note.chars().count() <= 500
            && self
                .target_cost
                .is_none_or(|cost| (0.0..1e9).contains(&cost))
    }
}

impl WorkspaceStore<'_> {
    pub async fn profile(&self, ad_account: &str) -> Result<Profile> {
        Ok(self
            .db
            .query_one_raw(sql(
                "SELECT goal,target_cost_micros,note FROM ad_account_profiles WHERE workspace_id=? AND ad_account_customer_id=?",
                vec![self.workspace.id().into(), ad_account.into()],
            ))
            .await?
            .map(|row| {
                Ok::<_, loco_rs::Error>(Profile {
                    goal: row.try_get("", "goal")?,
                    target_cost: row
                        .try_get::<Option<i64>>("", "target_cost_micros")?
                        .map(|micros| micros as f64 / 1_000_000.0),
                    note: row.try_get("", "note")?,
                })
            })
            .transpose()?
            .unwrap_or_default())
    }

    pub async fn save_profile(&self, ad_account: &str, profile: &Profile) -> Result<bool> {
        self.save_profile_on(self.db, ad_account, profile).await
    }

    pub(super) async fn save_profile_on(
        &self,
        db: &impl ConnectionTrait,
        ad_account: &str,
        profile: &Profile,
    ) -> Result<bool> {
        if !profile.valid() {
            return Ok(false);
        }
        Ok(db
            .execute_raw(sql(
                "INSERT INTO ad_account_profiles(workspace_id,ad_account_customer_id,goal,target_cost_micros,note) SELECT workspace_id,customer_id,?3,?4,?5 FROM ad_accounts WHERE workspace_id=?1 AND customer_id=?2 ON CONFLICT(workspace_id,ad_account_customer_id) DO UPDATE SET goal=excluded.goal,target_cost_micros=excluded.target_cost_micros,note=excluded.note",
                vec![
                    self.workspace.id().into(),
                    ad_account.into(),
                    profile.goal.clone().into(),
                    profile
                        .target_cost
                        .map(|cost| (cost * 1_000_000.0).round() as i64)
                        .into(),
                    profile.note.trim().into(),
                ],
            ))
            .await?
            .rows_affected()
            == 1)
    }
}
