pub mod admin;
mod entities;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use loco_rs::Result;
use rand::{rngs::OsRng, RngCore};
use sea_orm::{
    sea_query::OnConflict, ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseBackend,
    DatabaseConnection, EntityTrait, NotSet, PaginatorTrait, QueryFilter, Set, Statement,
    TransactionTrait, TryInsertResult,
};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{database::begin_write, language::Language, workspace::Workspace};
use entities::{oauth_states, sessions, users, workspaces};

pub const SESSION_SECONDS: i64 = 30 * 24 * 60 * 60;
pub const STATE_SECONDS: i64 = 10 * 60;

pub async fn workspace_ids(db: &DatabaseConnection) -> Result<Vec<String>> {
    Ok(workspaces::Entity::find()
        .all(db)
        .await?
        .into_iter()
        .map(|row| row.id)
        .collect())
}

/// Whether the User of the Workspace has a session that has not expired.
pub async fn signed_in(db: &DatabaseConnection, workspace_id: &str) -> Result<bool> {
    Ok(db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "SELECT 1 FROM sessions s JOIN workspaces w ON w.user_id=s.user_id WHERE w.id=? AND s.expires_at>?",
            vec![
                workspace_id.into(),
                crate::settings::wall_clock().timestamp().into(),
            ],
        ))
        .await?
        .is_some())
}

/// Deletes the Trial hashes of Trials that started before `oldest`, a Unix time.
pub async fn remove_trial_history_before(db: &DatabaseConnection, oldest: i64) -> Result<()> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Sqlite,
        "DELETE FROM trial_history WHERE started_at<?",
        vec![oldest.into()],
    ))
    .await?;
    Ok(())
}

#[derive(Serialize)]
pub struct User {
    pub id: String,
    pub email: String,
    pub name: String,
    pub language: Language,
}

pub struct SignedIn {
    pub user: User,
    pub workspace: Workspace,
    pub session_hash: String,
}

pub struct OAuthState {
    pub pkce_verifier: String,
    pub workspace_id: Option<String>,
    pub session_hash: Option<String>,
    pub sign_up_source: Option<SignUpSource>,
}

const SIGN_UP_SOURCES: [&str; 13] = [
    "nav",
    "home-nav",
    "home-hero",
    "home-price",
    "home-closing",
    "for-business-owners",
    "for-agency-clients",
    "for-marketing-managers",
    "resource",
    "home-nav-ro",
    "home-hero-ro",
    "home-price-ro",
    "home-closing-ro",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignUpSource(String);

impl SignUpSource {
    pub fn parse(value: &str) -> Option<Self> {
        let resource = value.strip_prefix("resource-").is_some_and(|digest| {
            digest.len() == 27
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        });
        (SIGN_UP_SOURCES.contains(&value) || resource).then(|| Self(value.to_owned()))
    }

    pub fn resource(path: &str) -> Self {
        Self(format!("resource-{}", &hash_token(path)[..27]))
    }

    pub fn is_resource(&self) -> bool {
        self.0.starts_with("resource-")
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub fn random_token() -> String {
    let mut bytes = [0; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn subject_hash(subject: &str) -> String {
    hash_token(subject)
}

pub fn hash_token(token: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(token.as_bytes()))
}

pub async fn sign_in(
    db: &DatabaseConnection,
    subject: &str,
    email: &str,
    name: &str,
    trial_start: i64,
    source: Option<SignUpSource>,
) -> Result<String> {
    let txn = db.begin().await?;
    users::Entity::insert(users::ActiveModel {
        id: Set(random_token()),
        google_subject_id: Set(subject.into()),
        email: Set(email.into()),
        name: Set(name.into()),
        language: NotSet,
    })
    .on_conflict(
        OnConflict::column(users::Column::GoogleSubjectId)
            .update_columns([users::Column::Email, users::Column::Name])
            .to_owned(),
    )
    .exec(&txn)
    .await?;
    let user = users::Entity::find()
        .filter(users::Column::GoogleSubjectId.eq(subject))
        .one(&txn)
        .await?
        .ok_or_else(|| loco_rs::Error::Message("User was not saved".into()))?;
    let workspace_id = random_token();
    let inserted = workspaces::Entity::insert(workspaces::ActiveModel {
        id: Set(workspace_id.clone()),
        user_id: Set(user.id.clone()),
        created_at: Set(Some(crate::settings::wall_clock().timestamp())),
        sign_up_source: Set(source.map(|source| source.as_str().to_owned())),
    })
    .on_conflict(
        OnConflict::column(workspaces::Column::UserId)
            .do_nothing()
            .to_owned(),
    )
    .try_insert()
    .exec(&txn)
    .await?;
    if let TryInsertResult::Inserted(_) = inserted {
        let remembered = txn
            .query_one_raw(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "SELECT started_at FROM trial_history WHERE subject_hash=?",
                vec![subject_hash(subject).into()],
            ))
            .await?
            .map(|row| row.try_get::<i64>("", "started_at"))
            .transpose()?;
        txn.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "UPDATE workspace_trials SET started_at=?,started=1 WHERE workspace_id=?",
            vec![
                remembered.unwrap_or(trial_start).into(),
                workspace_id.into(),
            ],
        ))
        .await?;
    }
    sessions::Entity::delete_many()
        .filter(sessions::Column::ExpiresAt.lte(crate::settings::wall_clock().timestamp()))
        .exec(&txn)
        .await?;
    let token = random_token();
    sessions::ActiveModel {
        token_hash: Set(hash_token(&token)),
        user_id: Set(user.id),
        expires_at: Set(crate::settings::wall_clock().timestamp() + SESSION_SECONDS),
    }
    .insert(&txn)
    .await?;
    txn.commit().await?;
    Ok(token)
}

pub async fn authenticate(db: &DatabaseConnection, token: &str) -> Result<Option<SignedIn>> {
    let session_hash = hash_token(token);
    let Some(row) = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "SELECT u.id AS user_id,u.email AS email,u.name AS name,u.language AS language,w.id AS workspace_id FROM sessions s JOIN users u ON u.id=s.user_id JOIN workspaces w ON w.user_id=u.id WHERE s.token_hash=? AND s.expires_at>?",
            vec![session_hash.clone().into(), crate::settings::wall_clock().timestamp().into()],
        ))
        .await?
    else {
        return Ok(None);
    };
    Ok(Some(SignedIn {
        user: User {
            id: row.try_get("", "user_id")?,
            email: row.try_get("", "email")?,
            name: row.try_get("", "name")?,
            language: Language::from_code(&row.try_get::<String>("", "language")?)
                .ok_or_else(|| loco_rs::Error::Message("User language is not valid".into()))?,
        },
        workspace: Workspace::from_identity(row.try_get("", "workspace_id")?),
        session_hash,
    }))
}

pub async fn sign_out(db: &DatabaseConnection, token: &str) -> Result<()> {
    sessions::Entity::delete_by_id(hash_token(token))
        .exec(db)
        .await?;
    Ok(())
}

pub const MAX_PENDING_STATES: u64 = 1_000;

pub async fn start_oauth(
    db: &DatabaseConnection,
    browser: &str,
    signed_in: Option<&SignedIn>,
    source: Option<SignUpSource>,
) -> Result<(String, String)> {
    let state = random_token();
    let verifier = random_token();
    let txn = begin_write(db).await?;
    oauth_states::Entity::delete_many()
        .filter(oauth_states::Column::ExpiresAt.lte(crate::settings::wall_clock().timestamp()))
        .exec(&txn)
        .await?;
    oauth_states::Entity::delete_many()
        .filter(oauth_states::Column::BrowserHash.eq(hash_token(browser)))
        .exec(&txn)
        .await?;
    let over =
        (oauth_states::Entity::find().count(&txn).await? + 1).saturating_sub(MAX_PENDING_STATES);
    if over > 0 {
        txn.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "DELETE FROM oauth_states WHERE state_hash IN (SELECT state_hash FROM oauth_states ORDER BY expires_at LIMIT ?)",
            vec![over.into()],
        ))
        .await?;
    }
    oauth_states::ActiveModel {
        state_hash: Set(hash_token(&state)),
        browser_hash: Set(hash_token(browser)),
        pkce_verifier: Set(verifier.clone()),
        workspace_id: Set(signed_in.map(|value| value.workspace.id().to_owned())),
        session_hash: Set(signed_in.map(|value| value.session_hash.clone())),
        sign_up_source: Set(source.map(|source| source.as_str().to_owned())),
        expires_at: Set(crate::settings::wall_clock().timestamp() + STATE_SECONDS),
    }
    .insert(&txn)
    .await?;
    txn.commit().await?;
    Ok((state, verifier))
}

pub async fn consume_oauth(
    db: &DatabaseConnection,
    state: &str,
    browser: &str,
) -> Result<Option<OAuthState>> {
    let state_hash = hash_token(state);
    let row = oauth_states::Entity::find_by_id(&state_hash)
        .filter(oauth_states::Column::BrowserHash.eq(hash_token(browser)))
        .filter(oauth_states::Column::ExpiresAt.gt(crate::settings::wall_clock().timestamp()))
        .one(db)
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let deleted = oauth_states::Entity::delete_by_id(state_hash)
        .exec(db)
        .await?;
    if deleted.rows_affected != 1 {
        return Ok(None);
    }
    Ok(Some(OAuthState {
        pkce_verifier: row.pkce_verifier,
        workspace_id: row.workspace_id,
        session_hash: row.session_hash,
        sign_up_source: row.sign_up_source.as_deref().and_then(SignUpSource::parse),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_resource_has_a_stable_source_that_fits_the_stored_limit() {
        let source = SignUpSource::resource("/guides/example-resource");
        assert_eq!(source.as_str(), "resource-sanDgL4_1-xs6W_4hI6kVvCrSmZ");
        assert_eq!(SignUpSource::parse(source.as_str()), Some(source));
        for source in SIGN_UP_SOURCES {
            assert_eq!(SignUpSource::parse(source).unwrap().as_str(), source);
        }
        for source in [
            "resource-",
            "resource-sanDgL4_1-xs6W_4hI6kVvCrSm",
            "resource-sanDgL4_1-xs6W_4hI6kVvCrSmZZ",
            "resource-sanDgL4_1-xs6W_4hI6kVvCrSm!",
            "resource-sanDgL4_1-xs6W_4hI6kVvCrSmé",
            "home-hero\"><script>",
            "https://example.test",
        ] {
            assert_eq!(SignUpSource::parse(source), None, "{source}");
        }
    }
}
