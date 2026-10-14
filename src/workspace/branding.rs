use loco_rs::Result;
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement, Value};
use serde::Serialize;

use super::WorkspaceStore;

fn sql(query: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Sqlite, query, values)
}

#[derive(Clone, Serialize)]
pub struct Branding {
    pub name: String,
    pub accent_colour: String,
    pub has_logo: bool,
}

const MAX_LOGO_BYTES: usize = 500 * 1024;

pub fn valid(name: &str, colour: &str) -> bool {
    !name.trim().is_empty()
        && name.chars().count() <= 100
        && colour.len() == 7
        && colour.starts_with('#')
        && colour[1..].bytes().all(|c| c.is_ascii_hexdigit())
}

impl WorkspaceStore<'_> {
    pub async fn branding(&self, ad_account: &str) -> Result<Option<Branding>> {
        let row = self.db.query_one_raw(sql(
            "SELECT COALESCE(b.name,a.name) AS name, COALESCE(b.accent_colour,'#0F766E') AS accent_colour, b.logo IS NOT NULL AS has_logo FROM ad_accounts a LEFT JOIN branding b ON b.workspace_id=a.workspace_id AND b.ad_account_customer_id=a.customer_id WHERE a.workspace_id=? AND a.customer_id=?",
            vec![self.workspace.id().into(), ad_account.into()],
        )).await?;
        row.map(|row| {
            Ok(Branding {
                name: row.try_get("", "name")?,
                accent_colour: row.try_get("", "accent_colour")?,
                has_logo: row.try_get("", "has_logo")?,
            })
        })
        .transpose()
    }

    pub async fn save_branding(&self, ad_account: &str, name: &str, colour: &str) -> Result<bool> {
        self.save_branding_on(self.db, ad_account, name, colour)
            .await
    }

    pub(super) async fn save_branding_on(
        &self,
        db: &impl ConnectionTrait,
        ad_account: &str,
        name: &str,
        colour: &str,
    ) -> Result<bool> {
        if !valid(name, colour) {
            return Ok(false);
        }
        let affected = db.execute_raw(sql(
            "INSERT INTO branding(workspace_id,ad_account_customer_id,name,accent_colour) SELECT workspace_id,customer_id,?,? FROM ad_accounts WHERE workspace_id=? AND customer_id=? ON CONFLICT(workspace_id,ad_account_customer_id) DO UPDATE SET name=excluded.name,accent_colour=excluded.accent_colour",
            vec![name.trim().into(), colour.to_uppercase().into(), self.workspace.id().into(), ad_account.into()],
        )).await?.rows_affected();
        Ok(affected == 1)
    }

    pub async fn logo(&self, ad_account: &str) -> Result<Option<Vec<u8>>> {
        Ok(self
            .db
            .query_one_raw(sql(
                "SELECT logo FROM branding WHERE workspace_id=? AND ad_account_customer_id=?",
                vec![self.workspace.id().into(), ad_account.into()],
            ))
            .await?
            .map(|row| row.try_get::<Option<Vec<u8>>>("", "logo"))
            .transpose()?
            .flatten())
    }

    pub async fn save_logo(&self, ad_account: &str, logo: &[u8]) -> Result<bool> {
        self.save_logo_on(self.db, ad_account, logo).await
    }

    pub(super) async fn save_logo_on(
        &self,
        db: &impl ConnectionTrait,
        ad_account: &str,
        logo: &[u8],
    ) -> Result<bool> {
        if logo.len() > MAX_LOGO_BYTES {
            return Ok(false);
        }
        Ok(db
            .execute_raw(sql(
                "UPDATE branding SET logo=? WHERE workspace_id=? AND ad_account_customer_id=?",
                vec![
                    logo.to_vec().into(),
                    self.workspace.id().into(),
                    ad_account.into(),
                ],
            ))
            .await?
            .rows_affected()
            == 1)
    }
}
