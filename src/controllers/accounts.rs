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
use serde_json::json;

use crate::{
    auth::Authenticated,
    google::{self, Refreshed},
    google_ads,
    language::{say, say_plural},
    monitoring,
    monitoring::access,
    settings::Settings,
    workspace::{Selection, WorkspaceStore},
};

async fn moved() -> Redirect {
    Redirect::to("/ad-accounts")
}

async fn connect(
    Authenticated(_signed): Authenticated,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    format::render().view(&v, "accounts/connect.html", data!({}))
}

fn display_id(customer_id: &str) -> String {
    match (
        customer_id.get(..3),
        customer_id.get(3..6),
        customer_id.get(6..),
    ) {
        (Some(a), Some(b), Some(c)) => format!("{a}-{b}-{c}"),
        _ => customer_id.to_owned(),
    }
}

struct Refusal {
    message: String,
}

async fn choose_page(
    v: &TeraView,
    store: &WorkspaceStore<'_>,
    selection: &Selection,
    checked: &[String],
    refusal: Option<Refusal>,
) -> Result<Response> {
    let added = store
        .ad_accounts()
        .await?
        .into_iter()
        .map(|ad_account| ad_account.customer_id)
        .collect::<Vec<_>>();
    let choices = selection
        .choices
        .iter()
        .map(|choice| {
            json!({
                "customer_id": choice.customer_id,
                "name": choice.name,
                "display_id": display_id(&choice.customer_id),
                "added": added.contains(&choice.customer_id),
                "checked": checked.contains(&choice.customer_id),
            })
        })
        .collect::<Vec<_>>();
    let refused = refusal.is_some();
    let mut response = format::render().view(
        v,
        "accounts/select.html",
        json!({
            "selection_id": selection.id,
            "choices": choices,
            "first": added.is_empty(),
            "limit": access::AD_ACCOUNT_LIMIT,
            "error": refusal.map(|refusal| refusal.message),
        }),
    )?;
    if refused {
        *response.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
    }
    Ok(response)
}

async fn select(
    Authenticated(signed_in): Authenticated,
    State(ctx): State<AppContext>,
    Path(id): Path<String>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    let store = WorkspaceStore::new(&ctx.db, &signed_in.workspace);
    let Some(selection) = store.selection(&id).await? else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let only = match selection.choices.as_slice() {
        [only] => vec![only.customer_id.clone()],
        _ => Vec::new(),
    };
    choose_page(&v, &store, &selection, &only, None).await
}

async fn discover(
    Authenticated(signed_in): Authenticated,
    State(ctx): State<AppContext>,
    Extension(settings): Extension<Arc<Settings>>,
    Path(connection_id): Path<String>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    let text = signed_in.user.language.text()?;
    let store = WorkspaceStore::new(&ctx.db, &signed_in.workspace);
    let Some(key) = settings.master_key.as_ref() else {
        return error_page(
            &v,
            StatusCode::SERVICE_UNAVAILABLE,
            text.written.page_google_connections_not_configured,
        );
    };
    let Some(refresh_token) = store.refresh_token(&connection_id, key).await? else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let choices = match google::refresh(&settings, &refresh_token).await {
        Ok(Refreshed::AccessToken(access_token)) => {
            google::discover_for_workspace(&settings, &access_token, &store).await
        }
        Ok(Refreshed::Revoked) => {
            return error_page(
                &v,
                StatusCode::CONFLICT,
                text.written.page_google_permission_refused,
            )
        }
        Err(error) => Err(error),
    };
    let Ok(choices) = choices else {
        return error_page(&v, StatusCode::BAD_GATEWAY, text.written.page_google_failed);
    };
    let selection = store.create_selection(&connection_id, choices).await?;
    Ok(Redirect::to(&format!("/ad-accounts/choose/{selection}")).into_response())
}

fn error_page(v: &TeraView, status: StatusCode, message: &str) -> Result<Response> {
    let mut response = format::render().view(v, "auth/error.html", data!({"message": message}))?;
    *response.status_mut() = status;
    Ok(response)
}

#[derive(Deserialize)]
struct Selected {
    #[serde(default)]
    ad_account: Vec<String>,
}

async fn save(
    Authenticated(signed_in): Authenticated,
    State(ctx): State<AppContext>,
    Extension(settings): Extension<Arc<Settings>>,
    ViewEngine(v): ViewEngine<TeraView>,
    Path(id): Path<String>,
    Form(form): Form<Selected>,
) -> Result<Response> {
    let text = signed_in.user.language.text()?;
    let store = WorkspaceStore::new(&ctx.db, &signed_in.workspace);
    let Some(selection) = store.selection(&id).await? else {
        return Ok(StatusCode::BAD_REQUEST.into_response());
    };
    let mut selected = form.ad_account;
    selected.sort();
    selected.dedup();
    let refuse = |message: String| Some(Refusal { message });
    let limit = access::AD_ACCOUNT_LIMIT.to_string();
    let existing = store.ad_accounts().await?;
    let total = existing
        .iter()
        .map(|ad| ad.customer_id.clone())
        .chain(selected.iter().cloned())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let refusal = if selected.is_empty() {
        refuse(text.written.page_choose_one_ad_account.into())
    } else if total > access::AD_ACCOUNT_LIMIT {
        refuse(if existing.is_empty() {
            say_plural!(text, page_ad_accounts_chosen, total as u64, limit = &limit)?
        } else {
            say!(
                text,
                page_ad_accounts_would_have,
                total = total,
                limit = &limit
            )?
        })
    } else if !store
        .monitoring_can_select_accounts(&selected, settings.now().timestamp())
        .await?
    {
        refuse(text.written.page_trial_ended_add.into())
    } else {
        None
    };
    if refusal.is_some() {
        return choose_page(&v, &store, &selection, &selected, refusal).await;
    }
    if !store.save_selected(&id, &selected).await? {
        return Ok(StatusCode::BAD_REQUEST.into_response());
    }
    if let [only] = selected.as_slice() {
        let collected = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            google_ads::collect_recent_history(&settings, &store, only),
        )
        .await;
        if !matches!(collected, Ok(Ok(()))) {
            tracing::warn!("The first Account history was not collected before the page");
        }
    }
    if monitoring::schedule::start_first_runs(&ctx, &signed_in.workspace, &selected)
        .await
        .is_err()
    {
        tracing::warn!("The first run could not be queued for new Ad Accounts");
    }
    Ok(Redirect::to(&match selected.as_slice() {
        [only] => format!("/ad-accounts/{only}"),
        _ => "/ad-accounts".to_owned(),
    })
    .into_response())
}

pub fn routes() -> Routes {
    Routes::new()
        .add("/accounts", get(moved))
        .add("/connect", get(connect))
        .add("/ad-accounts/choose/{id}", get(select).post(save))
        .add("/connections/{id}/ad-accounts", post(discover))
}
