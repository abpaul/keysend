use loco_rs::Result;
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement, Value};
use serde::Serialize;

use super::WorkspaceStore;
use crate::monitoring::economics::Economics;

fn sql(query: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Sqlite, query, values)
}

#[derive(Clone, Default, Serialize)]
pub struct Profile {
    pub goal: String,
    pub target_cost: Option<f64>,
    pub note: String,
    pub economics: Economics,
}

fn share(value: Option<f64>) -> bool {
    value.is_none_or(|share| share > 0.0 && share <= 1.0)
}

fn scaled(value: Option<f64>, scale: f64) -> sea_orm::Value {
    value.map(|value| (value * scale).round() as i64).into()
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
            && share(self.economics.margin)
            && share(self.economics.close_rate)
            && self
                .economics
                .customer_value
                .is_none_or(|value| value > 0.0 && value < 1e9)
            && self
                .economics
                .target_return
                .is_none_or(|target| target > 0.0 && target < 1e4)
    }
}

impl WorkspaceStore<'_> {
    pub async fn profile(&self, ad_account: &str) -> Result<Profile> {
        Ok(self
            .db
            .query_one_raw(sql(
                "SELECT goal,target_cost_micros,note,margin_basis_points,customer_value_micros,close_rate_basis_points,target_return_hundredths FROM ad_account_profiles WHERE workspace_id=? AND ad_account_customer_id=?",
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
                    economics: Economics {
                        margin: row
                            .try_get::<Option<i64>>("", "margin_basis_points")?
                            .map(|points| points as f64 / 10_000.0),
                        customer_value: row
                            .try_get::<Option<i64>>("", "customer_value_micros")?
                            .map(|micros| micros as f64 / 1_000_000.0),
                        close_rate: row
                            .try_get::<Option<i64>>("", "close_rate_basis_points")?
                            .map(|points| points as f64 / 10_000.0),
                        target_return: row
                            .try_get::<Option<i64>>("", "target_return_hundredths")?
                            .map(|hundredths| hundredths as f64 / 100.0),
                    },
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
                "INSERT INTO ad_account_profiles(workspace_id,ad_account_customer_id,goal,target_cost_micros,note,margin_basis_points,customer_value_micros,close_rate_basis_points,target_return_hundredths) SELECT workspace_id,customer_id,?3,?4,?5,?6,?7,?8,?9 FROM ad_accounts WHERE workspace_id=?1 AND customer_id=?2 ON CONFLICT(workspace_id,ad_account_customer_id) DO UPDATE SET goal=excluded.goal,target_cost_micros=excluded.target_cost_micros,note=excluded.note,margin_basis_points=excluded.margin_basis_points,customer_value_micros=excluded.customer_value_micros,close_rate_basis_points=excluded.close_rate_basis_points,target_return_hundredths=excluded.target_return_hundredths",
                vec![
                    self.workspace.id().into(),
                    ad_account.into(),
                    profile.goal.clone().into(),
                    profile
                        .target_cost
                        .map(|cost| (cost * 1_000_000.0).round() as i64)
                        .into(),
                    profile.note.trim().into(),
                    scaled(profile.economics.margin, 10_000.0),
                    scaled(profile.economics.customer_value, 1_000_000.0),
                    scaled(profile.economics.close_rate, 10_000.0),
                    scaled(profile.economics.target_return, 100.0),
                ],
            ))
            .await?
            .rows_affected()
            == 1)
    }
}
