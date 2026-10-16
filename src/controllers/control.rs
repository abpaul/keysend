use std::sync::Arc;

use axum::{
    extract::Path,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Redirect},
    Extension,
};
use axum_extra::extract::Form;
use loco_rs::prelude::*;
use serde::Deserialize;

use crate::{
    auth::Authenticated,
    language::Language,
    monitoring::worker::{TranslationJob, TranslationWorker},
    removal::Refusal,
    settings::Settings,
    workspace::WorkspaceStore,
};

#[derive(Deserialize)]
struct Confirmation {
    confirm: String,
}

#[derive(Deserialize)]
struct LanguageChoice {
    language: String,
}

async fn settings(
    Authenticated(signed): Authenticated,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    let store = WorkspaceStore::new(&ctx.db, &signed.workspace);
    let languages = Language::available()
        .into_iter()
        .map(|language| {
            data!({
                "code": language.code(),
                "name": language.name(),
                "selected": language == signed.user.language,
            })
        })
        .collect::<Vec<_>>();
    format::render().view(
        &v,
        "accounts/settings.html",
        data!({
            "email": signed.user.email,
            "languages": languages,
            "google_connections": store.google_connections().await?,
            "workspace_id": signed.workspace.id(),
        }),
    )
}

async fn choose_language(
    Authenticated(signed): Authenticated,
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Form(form): Form<LanguageChoice>,
) -> Result<Response> {
    let Some(language) = Language::parse_available(&form.language) else {
        return Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            signed.user.language.text()?.language_not_offered,
        )
            .into_response());
    };
    WorkspaceStore::new(&ctx.db, &signed.workspace)
        .set_user_language(language)
        .await?;
    if language != signed.user.language && language != Language::En {
        let job = TranslationJob {
            workspace_id: signed.workspace.id().to_owned(),
        };
        if TranslationWorker::perform_later(&ctx, job).await.is_err() {
            tracing::warn!("The translation of the stored texts could not be queued");
        }
    }
    Ok(Redirect::to(page_of(&headers).unwrap_or("/settings")).into_response())
}

fn page_of(headers: &HeaderMap) -> Option<&str> {
    let referer = headers.get(header::REFERER)?.to_str().ok()?;
    let (_, rest) = referer.split_once("://")?;
    let path = &rest[rest.find('/')?..];
    (!matches!(path.as_bytes().get(1), Some(b'/' | b'\\'))).then_some(path)
}

async fn confirm(
    Authenticated(signed): Authenticated,
    State(ctx): State<AppContext>,
    Path((kind, id)): Path<(String, String)>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    let store = WorkspaceStore::new(&ctx.db, &signed.workspace);
    let label = match kind.as_str() {
        "connections" => store
            .google_connections()
            .await?
            .into_iter()
            .find(|connection| connection.id == id)
            .map(|connection| connection.email),
        "workspaces" if id == signed.workspace.id() => Some(signed.user.email.clone()),
        _ => None,
    };
    let Some(label) = label else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    format::render().view(
        &v,
        "accounts/remove.html",
        data!({"kind":kind,"id":id,"label":label}),
    )
}

async fn remove(
    Authenticated(signed): Authenticated,
    State(ctx): State<AppContext>,
    Extension(settings): Extension<Arc<Settings>>,
    Path((kind, id)): Path<(String, String)>,
    Form(form): Form<Confirmation>,
) -> Result<Response> {
    if kind == "accounts" {
        return Ok(StatusCode::NOT_FOUND.into_response());
    }
    if let Some(refused) = remove_owned(
        &ctx,
        &settings,
        &signed,
        &kind,
        &id,
        form.confirm == "remove",
    )
    .await?
    {
        return Ok(refused);
    }
    Ok(Redirect::to(if kind == "workspaces" {
        "/"
    } else {
        "/ad-accounts"
    })
    .into_response())
}

pub(crate) async fn remove_owned(
    ctx: &AppContext,
    settings: &Settings,
    signed: &crate::identity::SignedIn,
    kind: &str,
    id: &str,
    confirmed: bool,
) -> Result<Option<Response>> {
    let store = WorkspaceStore::new(&ctx.db, &signed.workspace);
    let accounts = store.ad_accounts().await?;
    let connections = store.google_connections().await?;
    let owned = match kind {
        "accounts" => accounts.iter().any(|ad| ad.customer_id == id),
        "connections" => connections.iter().any(|connection| connection.id == id),
        "workspaces" => signed.workspace.id() == id,
        _ => false,
    };
    if !owned {
        return Ok(Some(StatusCode::NOT_FOUND.into_response()));
    }
    if !confirmed {
        return Ok(Some(StatusCode::UNPROCESSABLE_ENTITY.into_response()));
    }
    let refusal = match kind {
        "workspaces" => crate::removal::delete_workspace(ctx, settings, &signed.workspace).await?,
        "connections" => {
            let refusal = crate::removal::revoke_google(settings, &store, Some(id)).await?;
            if refusal.is_none() {
                store.remove_google_connection(id).await?;
            }
            refusal
        }
        _ => {
            store.remove_ad_account(id).await?;
            None
        }
    };
    refusal.map(|refusal| refused(refusal, signed)).transpose()
}

fn refused(refusal: Refusal, signed: &crate::identity::SignedIn) -> Result<Response> {
    let written = &signed.user.language.text()?.written;
    Ok(match refusal {
        Refusal::PaymentDetailsKept => (
            StatusCode::SERVICE_UNAVAILABLE,
            written.page_payment_details_not_removed,
        )
            .into_response(),
        Refusal::GooglePermissionKept => (
            StatusCode::SERVICE_UNAVAILABLE,
            written.page_google_permission_not_removed,
        )
            .into_response(),
        Refusal::MasterKeyMissing => StatusCode::SERVICE_UNAVAILABLE.into_response(),
        Refusal::RefreshTokenMissing => StatusCode::NOT_FOUND.into_response(),
    })
}

pub fn routes() -> Routes {
    Routes::new()
        .add("/settings", get(settings))
        .add("/settings/language", post(choose_language))
        .add("/remove/{kind}/{id}", get(confirm).post(remove))
}
