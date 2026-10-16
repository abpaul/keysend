use loco_rs::{app::AppContext, Result};

use crate::{
    settings::Settings,
    workspace::{Workspace, WorkspaceStore},
};

/// A reason why KeySend kept the data: an outside service did not confirm, or a secret is missing.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    PaymentDetailsKept,
    GooglePermissionKept,
    MasterKeyMissing,
    RefreshTokenMissing,
}

/// Revokes the refresh token of one Google Connection, or of every Google Connection of the
/// Workspace when `only` is `None`.
pub async fn revoke_google(
    settings: &Settings,
    store: &WorkspaceStore<'_>,
    only: Option<&str>,
) -> Result<Option<Refusal>> {
    let Some(key) = settings.master_key.as_ref() else {
        return Ok(Some(Refusal::MasterKeyMissing));
    };
    for connection in store
        .google_connections()
        .await?
        .iter()
        .filter(|connection| only.is_none_or(|id| connection.id == id))
    {
        let Some(token) = store.refresh_token(&connection.id, key).await? else {
            return Ok(Some(Refusal::RefreshTokenMissing));
        };
        if crate::google::revoke(settings, &token).await.is_err() {
            return Ok(Some(Refusal::GooglePermissionKept));
        }
    }
    Ok(None)
}

async fn delete_stripe_customer(
    settings: &Settings,
    store: &WorkspaceStore<'_>,
) -> Result<Option<Refusal>> {
    let Some(customer) = store.stripe_customer().await? else {
        return Ok(None);
    };
    let deleted = crate::billing::request(
        settings,
        reqwest::Method::DELETE,
        &format!("customers/{customer}"),
        &[],
        None,
    )
    .await;
    Ok(
        if matches!(deleted, Ok(body) if body["id"]==customer && body["deleted"]==true) {
            None
        } else {
            Some(Refusal::PaymentDetailsKept)
        },
    )
}

/// Revokes every Google refresh token, deletes the Stripe customer, records the Trial hash, and
/// deletes the User and the Workspace. When Google or Stripe does not confirm, it deletes no row.
/// Google comes first because a revoke of a revoked token succeeds, so a retry after a Stripe
/// failure repeats nothing that can fail.
pub async fn delete_workspace(
    ctx: &AppContext,
    settings: &Settings,
    workspace: &Workspace,
) -> Result<Option<Refusal>> {
    let store = WorkspaceStore::new(&ctx.db, workspace);
    if let Some(refusal) = revoke_google(settings, &store, None).await? {
        return Ok(Some(refusal));
    }
    if let Some(refusal) = delete_stripe_customer(settings, &store).await? {
        return Ok(Some(refusal));
    }
    store.delete_user().await?;
    Ok(None)
}
