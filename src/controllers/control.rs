use std::sync::Arc;

use axum::{
    extract::Path,
    http::StatusCode,
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
    Ok(Redirect::to("/settings").into_response())
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
    if kind == "workspaces" {
        if let Some(customer) = store.stripe_customer().await? {
            let deleted = crate::billing::request(
                settings,
                reqwest::Method::DELETE,
                &format!("customers/{customer}"),
                &[],
                None,
            )
            .await;
            if !matches!(deleted, Ok(body) if body["id"]==customer && body["deleted"]==true) {
                return Ok(Some(
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        signed
                            .user
                            .language
                            .text()?
                            .written
                            .page_payment_details_not_removed,
                    )
                        .into_response(),
                ));
            }
        }
    }
    if kind != "accounts" {
        let Some(key) = settings.master_key.as_ref() else {
            return Ok(Some(StatusCode::SERVICE_UNAVAILABLE.into_response()));
        };
        for connection in connections
            .iter()
            .filter(|connection| kind == "workspaces" || connection.id == id)
        {
            let Some(token) = store.refresh_token(&connection.id, key).await? else {
                return Ok(Some(StatusCode::NOT_FOUND.into_response()));
            };
            if crate::google::revoke(settings, &token).await.is_err() {
                return Ok(Some(
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        signed
                            .user
                            .language
                            .text()?
                            .written
                            .page_google_permission_not_removed,
                    )
                        .into_response(),
                ));
            }
        }
    }
    match kind {
        "accounts" => store.remove_ad_account(id).await?,
        "connections" => store.remove_google_connection(id).await?,
        _ => store.delete_user().await?,
    }
    Ok(None)
}

pub fn routes() -> Routes {
    Routes::new()
        .add("/settings", get(settings))
        .add("/settings/language", post(choose_language))
        .add("/remove/{kind}/{id}", get(confirm).post(remove))
}
