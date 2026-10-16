mod billing;
pub mod branding;
mod campaign_settings;
mod control;
mod entities;
mod google_ads;
pub mod history;
pub mod monitoring;
pub mod profile;
mod retention;
pub mod settings;

pub use google_ads::Operation;

use loco_rs::{Error, Result};
use sea_orm::{
    sea_query::OnConflict, ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseBackend,
    DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set, Statement, TransactionTrait,
};
use serde::{Deserialize, Serialize};

use crate::{crypto::MasterKey, database::begin_write, identity::random_token, language::Language};
use entities::{ad_account_selections, ad_accounts, google_connections};

pub struct Workspace {
    id: String,
}

impl Workspace {
    pub(crate) fn from_identity(id: String) -> Self {
        Self { id }
    }
    pub fn id(&self) -> &str {
        &self.id
    }
}

#[derive(Serialize)]
pub struct GoogleConnection {
    pub id: String,
    pub email: String,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct AdAccountChoice {
    pub customer_id: String,
    pub name: String,
    pub login_customer_id: Option<String>,
}

#[derive(Serialize)]
pub struct AdAccount {
    pub customer_id: String,
    pub name: String,
    pub google_connection_id: String,
    pub login_customer_id: Option<String>,
}

pub struct Selection {
    pub id: String,
    pub google_connection_id: String,
    pub choices: Vec<AdAccountChoice>,
}

pub struct WorkspaceStore<'a> {
    db: &'a DatabaseConnection,
    workspace: &'a Workspace,
}

impl<'a> WorkspaceStore<'a> {
    pub fn new(db: &'a DatabaseConnection, workspace: &'a Workspace) -> Self {
        Self { db, workspace }
    }

    pub async fn user_email(&self) -> Result<Option<String>> {
        self.db
            .query_one_raw(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "SELECT u.email FROM users u JOIN workspaces w ON w.user_id=u.id WHERE w.id=?",
                vec![self.workspace.id().into()],
            ))
            .await?
            .map(|row| Ok(row.try_get("", "email")?))
            .transpose()
    }

    pub async fn user_language(&self) -> Result<Language> {
        let row = self
            .db
            .query_one_raw(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "SELECT u.language FROM users u JOIN workspaces w ON w.user_id=u.id WHERE w.id=?",
                vec![self.workspace.id().into()],
            ))
            .await?
            .ok_or_else(|| Error::Message("The Workspace has no user".into()))?;
        Language::from_code(&row.try_get::<String>("", "language")?)
            .ok_or_else(|| Error::Message("User language is not valid".into()))
    }

    pub async fn set_user_language(&self, language: Language) -> Result<()> {
        self.db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "UPDATE users SET language=? WHERE id=(SELECT user_id FROM workspaces WHERE id=?)",
                vec![language.code().into(), self.workspace.id().into()],
            ))
            .await?;
        Ok(())
    }

    pub async fn google_connections(&self) -> Result<Vec<GoogleConnection>> {
        Ok(google_connections::Entity::find()
            .filter(google_connections::Column::WorkspaceId.eq(self.workspace.id()))
            .order_by_asc(google_connections::Column::Email)
            .all(self.db)
            .await?
            .into_iter()
            .map(|row| GoogleConnection {
                id: row.id,
                email: row.email,
            })
            .collect())
    }

    pub async fn ad_accounts(&self) -> Result<Vec<AdAccount>> {
        Ok(ad_accounts::Entity::find()
            .filter(ad_accounts::Column::WorkspaceId.eq(self.workspace.id()))
            .order_by_asc(ad_accounts::Column::Name)
            .all(self.db)
            .await?
            .into_iter()
            .map(|row| AdAccount {
                customer_id: row.customer_id,
                name: row.name,
                google_connection_id: row.google_connection_id,
                login_customer_id: row.login_customer_id,
            })
            .collect())
    }

    pub async fn save_google_connection(
        &self,
        subject: &str,
        email: &str,
        refresh_token: &str,
        key: &MasterKey,
    ) -> Result<GoogleConnection> {
        let txn = begin_write(self.db).await?;
        let existing = google_connections::Entity::find()
            .filter(google_connections::Column::WorkspaceId.eq(self.workspace.id()))
            .filter(google_connections::Column::GoogleSubjectId.eq(subject))
            .one(&txn)
            .await?;
        let id = existing.map_or_else(random_token, |row| row.id);
        let encrypted = key.encrypt(self.workspace.id(), &id, refresh_token)?;
        google_connections::Entity::insert(google_connections::ActiveModel {
            id: Set(id.clone()),
            workspace_id: Set(self.workspace.id().into()),
            google_subject_id: Set(subject.into()),
            email: Set(email.into()),
            encrypted_refresh_token: Set(encrypted),
        })
        .on_conflict(
            OnConflict::columns([
                google_connections::Column::WorkspaceId,
                google_connections::Column::GoogleSubjectId,
            ])
            .update_columns([
                google_connections::Column::Email,
                google_connections::Column::EncryptedRefreshToken,
            ])
            .to_owned(),
        )
        .exec(&txn)
        .await?;
        txn.commit().await?;
        Ok(GoogleConnection {
            id,
            email: email.into(),
        })
    }

    pub async fn refresh_token(
        &self,
        connection_id: &str,
        key: &MasterKey,
    ) -> Result<Option<String>> {
        let row = google_connections::Entity::find_by_id(connection_id)
            .filter(google_connections::Column::WorkspaceId.eq(self.workspace.id()))
            .one(self.db)
            .await?;
        row.map(|row| key.decrypt(self.workspace.id(), &row.id, &row.encrypted_refresh_token))
            .transpose()
    }

    pub async fn create_selection(
        &self,
        connection_id: &str,
        choices: Vec<AdAccountChoice>,
    ) -> Result<String> {
        if self
            .google_connections()
            .await?
            .iter()
            .all(|row| row.id != connection_id)
        {
            return Err(invalid_selection());
        }
        if choices.iter().any(|choice| {
            !valid_id(&choice.customer_id)
                || choice
                    .login_customer_id
                    .as_deref()
                    .is_some_and(|id| !valid_id(id))
        }) {
            return Err(invalid_selection());
        }
        let id = random_token();
        ad_account_selections::Entity::delete_many()
            .filter(ad_account_selections::Column::WorkspaceId.eq(self.workspace.id()))
            .filter(
                ad_account_selections::Column::ExpiresAt
                    .lte(crate::settings::wall_clock().timestamp()),
            )
            .exec(self.db)
            .await?;
        ad_account_selections::ActiveModel {
            id: Set(id.clone()),
            workspace_id: Set(self.workspace.id().into()),
            google_connection_id: Set(connection_id.into()),
            choices: Set(serde_json::to_string(&choices)?),
            expires_at: Set(crate::settings::wall_clock().timestamp() + 15 * 60),
        }
        .insert(self.db)
        .await?;
        Ok(id)
    }

    pub async fn selection(&self, id: &str) -> Result<Option<Selection>> {
        let row = ad_account_selections::Entity::find_by_id(id)
            .filter(ad_account_selections::Column::WorkspaceId.eq(self.workspace.id()))
            .filter(
                ad_account_selections::Column::ExpiresAt
                    .gt(crate::settings::wall_clock().timestamp()),
            )
            .one(self.db)
            .await?;
        row.map(|row| {
            Ok(Selection {
                id: row.id,
                google_connection_id: row.google_connection_id,
                choices: serde_json::from_str(&row.choices)?,
            })
        })
        .transpose()
    }

    pub async fn save_selected(&self, selection_id: &str, selected: &[String]) -> Result<bool> {
        let Some(selection) = self.selection(selection_id).await? else {
            return Ok(false);
        };
        if selected.iter().any(|id| {
            !selection
                .choices
                .iter()
                .any(|choice| &choice.customer_id == id)
        }) {
            return Ok(false);
        }
        let txn = self.db.begin().await?;
        let deleted = ad_account_selections::Entity::delete_many()
            .filter(ad_account_selections::Column::Id.eq(selection_id))
            .filter(ad_account_selections::Column::WorkspaceId.eq(self.workspace.id()))
            .filter(
                ad_account_selections::Column::ExpiresAt
                    .gt(crate::settings::wall_clock().timestamp()),
            )
            .exec(&txn)
            .await?;
        if deleted.rows_affected != 1 {
            return Ok(false);
        }
        let ids = ad_accounts::Entity::find()
            .filter(ad_accounts::Column::WorkspaceId.eq(self.workspace.id()))
            .all(&txn)
            .await?
            .into_iter()
            .map(|row| row.customer_id)
            .chain(selected.iter().cloned())
            .collect::<std::collections::BTreeSet<_>>();
        if ids.len() > crate::monitoring::access::AD_ACCOUNT_LIMIT {
            return Ok(false);
        }
        for choice in selection
            .choices
            .iter()
            .filter(|choice| selected.contains(&choice.customer_id))
        {
            ad_accounts::Entity::insert(ad_accounts::ActiveModel {
                id: Set(random_token()),
                workspace_id: Set(self.workspace.id().into()),
                google_connection_id: Set(selection.google_connection_id.clone()),
                customer_id: Set(choice.customer_id.clone()),
                name: Set(choice.name.clone()),
                login_customer_id: Set(choice.login_customer_id.clone()),
            })
            .on_conflict(
                OnConflict::columns([
                    ad_accounts::Column::WorkspaceId,
                    ad_accounts::Column::CustomerId,
                ])
                .update_columns([
                    ad_accounts::Column::GoogleConnectionId,
                    ad_accounts::Column::Name,
                    ad_accounts::Column::LoginCustomerId,
                ])
                .to_owned(),
            )
            .exec(&txn)
            .await?;
        }
        txn.commit().await?;
        Ok(true)
    }
}

pub fn valid_id(id: &str) -> bool {
    id.len() == 10 && id.bytes().all(|byte| byte.is_ascii_digit())
}
fn invalid_selection() -> Error {
    Error::Message("Invalid Ad Account selection".into())
}
