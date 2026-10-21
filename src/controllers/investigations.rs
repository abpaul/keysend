use std::sync::Arc;

use axum::{
    extract::Path,
    http::StatusCode,
    response::{IntoResponse, Redirect},
    Extension,
};
use axum_extra::extract::Form;
use loco_rs::{bgworker::BackgroundWorker, prelude::*};
use serde::Deserialize;
use serde_json::json;

use crate::{
    agent::{
        operation::Period,
        worker::{InvestigationJob, InvestigationWorker},
        Failure, State as InvestigationState,
    },
    auth::Authenticated,
    identity::SignedIn,
    language::Text,
    settings::Settings,
    workspace::{
        investigations::{InvestigationFinish, InvestigationUsage},
        WorkspaceStore,
    },
};

mod view;

#[derive(Default, Deserialize)]
struct Question {
    #[serde(default)]
    question: Vec<String>,
    #[serde(default)]
    accounts: Vec<String>,
    #[serde(default)]
    period: String,
    #[serde(skip)]
    follows: Option<i64>,
}

impl Question {
    fn question(&self) -> &str {
        self.question.last().map_or("", |question| question.trim())
    }
}

async fn index(
    Authenticated(signed): Authenticated,
    State(ctx): State<AppContext>,
    Extension(settings): Extension<Arc<Settings>>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    let store = WorkspaceStore::new(&ctx.db, &signed.workspace);
    let form = Question {
        accounts: store
            .ad_accounts()
            .await?
            .into_iter()
            .map(|ad| ad.customer_id)
            .collect(),
        period: "last_30_days".into(),
        ..Default::default()
    };
    entry(
        &v,
        &store,
        &settings,
        signed.user.language.text()?,
        &form,
        &[],
        StatusCode::OK,
    )
    .await
}

async fn reopening(store: &WorkspaceStore<'_>, settings: &Settings) -> Result<Option<String>> {
    let since = settings.now().timestamp() - 86_400;
    if store.investigations_created_since(since).await? < settings.agent.daily {
        return Ok(None);
    }
    let rows = store
        .recent_investigations(settings.agent.daily.clamp(1, u32::MAX as i64) as u32)
        .await?;
    let opens = rows
        .last()
        .map_or(settings.now().timestamp(), |row| row.created_at)
        + 86_400;
    Ok(Some(
        chrono::DateTime::from_timestamp(opens, 0)
            .unwrap_or(settings.now())
            .format("%Y-%m-%d %H:%M UTC")
            .to_string(),
    ))
}

async fn entry(
    v: &TeraView,
    store: &WorkspaceStore<'_>,
    settings: &Settings,
    text: &Text,
    form: &Question,
    errors: &[serde_json::Value],
    status: StatusCode,
) -> Result<Response> {
    let accounts = store.ad_accounts().await?;
    let recent = store.recent_investigations(30).await?;
    let recent = recent
        .iter()
        .map(|row| view::recent(row, &accounts, text))
        .collect::<Vec<_>>();
    let mut response = format::render().view(v, "investigations/index.html", json!({
        "accounts": accounts.iter().map(|ad| json!({"id":ad.customer_id,"name":ad.name,"checked":form.accounts.contains(&ad.customer_id)})).collect::<Vec<_>>(),
        "action":form.follows.map_or_else(|| "/investigations".to_owned(), |id| format!("/investigations/{id}/follow-ups")),
        "follow_up":form.follows.is_some(),
        "question": form.question(), "period":form.period, "errors":errors, "recent":recent,
        "access":store.monitoring_access(settings.now().timestamp()).await?, "reopens":reopening(store, settings).await?,
    }))?;
    *response.status_mut() = status;
    Ok(response)
}

async fn create(
    Authenticated(signed): Authenticated,
    State(ctx): State<AppContext>,
    Extension(settings): Extension<Arc<Settings>>,
    ViewEngine(v): ViewEngine<TeraView>,
    Form(form): Form<Question>,
) -> Result<Response> {
    submit(&ctx, &settings, &v, &signed, form, None).await
}

async fn submit(
    ctx: &AppContext,
    settings: &Settings,
    v: &TeraView,
    signed: &SignedIn,
    form: Question,
    follows: Option<i64>,
) -> Result<Response> {
    let store = WorkspaceStore::new(&ctx.db, &signed.workspace);
    let text = signed.user.language.text()?;
    if !store.monitoring_access(settings.now().timestamp()).await? {
        return entry(v, &store, settings, text, &form, &[], StatusCode::OK).await;
    }
    let accounts = store.ad_accounts().await?;
    let mut errors = Vec::new();
    if !(1..=crate::agent::investigation::QUESTION_LENGTH)
        .contains(&form.question().chars().count())
    {
        errors.push(json!({"field":"question","message":text.written.inv_invalid_question}));
    }
    if form.accounts.is_empty()
        || form.accounts.len() > 5
        || form
            .accounts
            .iter()
            .any(|id| !accounts.iter().any(|ad| &ad.customer_id == id))
    {
        errors.push(json!({"field":"accounts","message":text.written.inv_invalid_accounts}));
    }
    let period = match form.period.as_str() {
        "last_7_days" => Some(Period::Last7Days),
        "last_30_days" => Some(Period::Last30Days),
        _ => {
            errors.push(json!({"field":"period","message":text.written.inv_invalid_period}));
            None
        }
    };
    if !errors.is_empty() {
        return entry(
            v,
            &store,
            settings,
            text,
            &form,
            &errors,
            StatusCode::UNPROCESSABLE_ENTITY,
        )
        .await;
    }
    let mut selected = form.accounts.clone();
    let mut seen = std::collections::HashSet::new();
    selected.retain(|id| seen.insert(id.clone()));
    let Some(id) = store
        .create_investigation(
            form.question(),
            &selected,
            period.expect("validated Window"),
            follows,
            settings.now().timestamp(),
            settings.agent.daily,
            settings.now().timestamp() - 86_400,
        )
        .await?
    else {
        return entry(
            v,
            &store,
            settings,
            text,
            &form,
            &[],
            StatusCode::TOO_MANY_REQUESTS,
        )
        .await;
    };
    if let Err(error) = InvestigationWorker::perform_later(
        ctx,
        InvestigationJob {
            workspace_id: signed.workspace.id().into(),
            investigation_id: id,
        },
    )
    .await
    {
        tracing::warn!(%error, "Investigation could not be queued");
        store
            .finish_investigation(
                id,
                &InvestigationFinish {
                    state: InvestigationState::Failed,
                    failure: Some(Failure::Interrupted),
                    report: None,
                    usage: InvestigationUsage::default(),
                },
                settings.now().timestamp(),
            )
            .await?;
    }
    store
        .record_activity(crate::workspace::measurement::Activity::Viewed)
        .await;
    Ok(Redirect::to(&format!("/investigations/{id}")).into_response())
}

async fn show(
    Authenticated(signed): Authenticated,
    State(ctx): State<AppContext>,
    Path(id): Path<i64>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    let store = WorkspaceStore::new(&ctx.db, &signed.workspace);
    if store.investigation(id).await?.is_none() {
        return Ok(crate::app::not_found());
    }
    let rows = store.investigation_thread(id).await?;
    let accounts = store.ad_accounts().await?;
    let text = signed.user.language.text()?;
    let newest = rows.last().ok_or(Error::NotFound)?;
    let thread = rows
        .iter()
        .map(|row| view::thread(row, &accounts, text))
        .collect::<Result<Vec<_>>>()?;
    format::render().view(&v, "investigations/thread.html", json!({"thread":thread,"refresh":!newest.state.finished(),"latest":newest.id,"finished":newest.state.finished()}))
}

async fn follow_up(
    Authenticated(signed): Authenticated,
    State(ctx): State<AppContext>,
    Extension(settings): Extension<Arc<Settings>>,
    Path(id): Path<i64>,
    ViewEngine(v): ViewEngine<TeraView>,
    Form(mut form): Form<Question>,
) -> Result<Response> {
    let store = WorkspaceStore::new(&ctx.db, &signed.workspace);
    let Some(row) = store.investigation(id).await? else {
        return Ok(crate::app::not_found());
    };
    if !row.state.finished() {
        return Ok(StatusCode::CONFLICT.into_response());
    }
    form.follows = Some(id);
    form.accounts = row.accounts;
    form.period = match row.period {
        Period::Last7Days => "last_7_days",
        Period::Last30Days => "last_30_days",
    }
    .into();
    submit(&ctx, &settings, &v, &signed, form, Some(id)).await
}

async fn cancel(
    Authenticated(signed): Authenticated,
    State(ctx): State<AppContext>,
    Extension(settings): Extension<Arc<Settings>>,
    Path(id): Path<i64>,
) -> Result<Response> {
    let store = WorkspaceStore::new(&ctx.db, &signed.workspace);
    if store.investigation(id).await?.is_none() {
        return Ok(crate::app::not_found());
    }
    store
        .cancel_investigation(id, settings.now().timestamp())
        .await?;
    Ok(Redirect::to(&format!("/investigations/{id}")).into_response())
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/investigations")
        .add("/", get(index).post(create))
        .add("/{id}", get(show))
        .add("/{id}/follow-ups", post(follow_up))
        .add("/{id}/cancel", post(cancel))
}
