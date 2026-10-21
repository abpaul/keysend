//! Optimize: the Proposals of the Workspace, their Approval, the history, and the Optimization
//! policy of each Ad Account with its stop.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use axum::{
    extract::{Path, Query},
    http::StatusCode,
    response::Redirect,
    Extension,
};
use axum_extra::extract::Form;
use loco_rs::{bgworker::BackgroundWorker, prelude::*};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    auth::{Authenticated, OwnedAdAccount},
    language::Text,
    optimize::{
        cycle::{OptimizationJob, OptimizationWorker, Task},
        execute,
        policy::{self, Policy, Standing},
        propose::Decision,
        shadow::{Abstention, Feedback, Observation},
        ActionKind, Mode, Proposal, ProposalState,
    },
    settings::Settings,
    workspace::{
        optimization::{Event, Recovery, Report, NOT_APPLIED, RECOVERED},
        shadow::Shadow,
        AdAccount, WorkspaceStore,
    },
};

mod view;

use view::say;

const OPEN: [ProposalState; 1] = [ProposalState::Open];
const RUNNING: [ProposalState; 3] = [
    ProposalState::Approved,
    ProposalState::Applying,
    ProposalState::Uncertain,
];
const DONE: [ProposalState; 3] = [
    ProposalState::Applied,
    ProposalState::Failed,
    ProposalState::Conflict,
];

async fn currency(store: &WorkspaceStore<'_>, id: &str) -> Result<String> {
    Ok(store
        .ad_account_metadata(id)
        .await?
        .map_or_else(|| "USD".to_owned(), |(currency, _)| currency))
}

fn mode(text: &'static Text, mode: Mode) -> Result<String> {
    say(
        text,
        match mode {
            Mode::Investigate => "opt_mode_investigate",
            Mode::Shadow => "opt_mode_shadow",
            Mode::Recommend => "opt_mode_recommend",
            Mode::Autopilot => "opt_mode_autopilot",
        },
        &[],
    )
}

/// Whether this installation may run the change: an automatic one needs the Autopilot ceiling.
/// A budget raise never runs: KeySend cannot read the current budget total of the Ad Account.
fn installed(settings: &Settings, draft: &crate::optimize::Draft) -> bool {
    execute::live(settings, &draft.action)
        && !raises(&draft.action)
        && (draft.approval != crate::optimize::Approval::Automatic
            || settings.optimize.max_mode >= Mode::Autopilot)
}

fn raises(action: &crate::optimize::Action) -> bool {
    matches!(action, crate::optimize::Action::AdjustCampaignBudget { from_micros, to_micros, .. } if to_micros > from_micros)
}

/// Only Recommend and Autopilot let a change run; Investigate and Shadow never do.
fn changes(mode: Mode) -> bool {
    matches!(mode, Mode::Recommend | Mode::Autopilot)
}

fn standing(text: &'static Text, standing: Standing, reason: &str) -> Result<String> {
    Ok(match standing {
        Standing::Active => say(text, "opt_standing_active", &[])?,
        Standing::Revoked => say(text, "opt_standing_revoked", &[])?,
        Standing::Suspended => say(
            text,
            "opt_standing_suspended",
            &[(
                "reason",
                say(
                    text,
                    match reason {
                        "access" => "opt_suspend_access",
                        "failures" => "opt_suspend_failures",
                        "conflict" | "stale" | "shared_budget" => "opt_suspend_conflict",
                        "tracking" => "opt_suspend_tracking",
                        "sync" => "opt_suspend_sync",
                        "spend" => "opt_suspend_spend",
                        RECOVERED => "opt_suspend_recovered",
                        _ => "opt_suspend_other",
                    },
                    &[],
                )?,
            )],
        )?,
    })
}

/// One line of the history. A change that the User reported as not applied says so, and that
/// KeySend did not verify it.
fn event_words(text: &'static Text, event: &Event) -> Result<String> {
    let detail = &event.detail;
    match event.kind.as_str() {
        "failed" if detail["recovery"].as_str() == Some(NOT_APPLIED) => say(
            text,
            "opt_event_recovered",
            &[(
                "note",
                detail["note"].as_str().unwrap_or_default().to_owned(),
            )],
        ),
        "policy" if detail["reason"].as_str() == Some(RECOVERED) => say(
            text,
            "opt_event_recovered_policy",
            &[(
                "version",
                detail["version"].as_i64().unwrap_or_default().to_string(),
            )],
        ),
        _ => view::event(text, event),
    }
}

/// The context that every row of a list reads: names and currencies of the Ad Accounts.
struct Lookup {
    names: BTreeMap<String, String>,
    currencies: BTreeMap<String, String>,
}

impl Lookup {
    async fn load(store: &WorkspaceStore<'_>, accounts: &[AdAccount]) -> Result<Self> {
        Ok(Self {
            names: accounts
                .iter()
                .map(|ad| (ad.customer_id.clone(), ad.name.clone()))
                .collect(),
            currencies: store.ad_account_currencies().await?,
        })
    }

    fn name(&self, id: &str) -> String {
        self.names.get(id).cloned().unwrap_or_default()
    }

    fn currency(&self, id: &str) -> &str {
        self.currencies.get(id).map_or("USD", String::as_str)
    }
}

/// A Proposal as a row of a list.
fn row(text: &'static Text, lookup: &Lookup, proposal: &Proposal) -> Result<Value> {
    Ok(json!({
        "id": proposal.id,
        "account": lookup.name(&proposal.ad_account),
        "headline": view::headline(text, &proposal.draft, lookup.currency(&proposal.ad_account))?,
        "risk": view::risk(text, proposal.draft.action.risk())?,
        "high": proposal.draft.action.risk() == crate::optimize::Risk::High,
        "state": view::state(text, proposal.state)?,
        "automatic": proposal.draft.approval == crate::optimize::Approval::Automatic,
        "date": view::timestamp(proposal.draft.created_at),
    }))
}

async fn index(
    Authenticated(signed): Authenticated,
    State(ctx): State<AppContext>,
    Extension(settings): Extension<Arc<Settings>>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    let store = WorkspaceStore::new(&ctx.db, &signed.workspace);
    let text = signed.user.language.text()?;
    let accounts = store.ad_accounts().await?;
    let lookup = Lookup::load(&store, &accounts).await?;
    let events = store.optimization_events(None, None, 200).await?;
    let rows = |states: &[ProposalState], proposals: Vec<Proposal>| -> Result<Vec<Value>> {
        proposals
            .iter()
            .filter(|proposal| states.contains(&proposal.state))
            .map(|proposal| row(text, &lookup, proposal))
            .collect()
    };
    let open = rows(&OPEN, store.proposals(None, &OPEN, 50).await?)?;
    let running = rows(&RUNNING, store.proposals(None, &RUNNING, 20).await?)?;
    let mut done = Vec::new();
    for proposal in store.proposals(None, &DONE, 10).await? {
        let mut found = row(text, &lookup, &proposal)?;
        let outcome = events
            .iter()
            .find(|event| event.kind == "outcome" && event.proposal_id == Some(proposal.id))
            .and_then(|event| event.detail["state"].as_str());
        found["outcome"] = match outcome {
            Some(state) => json!(say(text, &format!("opt_outcome_{state}"), &[])?),
            None if proposal.state == ProposalState::Applied => {
                json!(say(text, "opt_outcome_awaiting_data", &[])?)
            }
            None => Value::Null,
        };
        done.push(found);
    }
    let policies = store.optimization_policies().await?;
    let evaluations = store.optimization_evaluations().await?;
    let mut accounts_rows = Vec::new();
    for ad in &accounts {
        let current = policies.get(&ad.customer_id);
        let policy_mode = current.map_or(Mode::Investigate, |current| current.policy.mode);
        let evaluation = evaluations.get(&ad.customer_id);
        let mut held = Vec::new();
        if let Some(holds) = evaluation.and_then(|(_, holds)| holds.as_object()) {
            for (hold, count) in holds {
                let words = view::hold(text, hold)?;
                if !words.is_empty() {
                    held.push(json!({"text": words, "count": count}));
                }
            }
        }
        accounts_rows.push(json!({
            "id": ad.customer_id,
            "name": ad.name,
            "mode": mode(text, policy_mode)?,
            "autopilot": policy_mode == Mode::Autopilot,
            "standing": match current { Some(current) => standing(text, current.standing, &current.reason)?, None => String::new() },
            "suspended": current.is_some_and(|current| current.standing == Standing::Suspended),
            "evaluated": evaluation.map(|(at, _)| view::timestamp(*at)),
            "held": held,
        }));
    }
    let mut shadows = Vec::new();
    for shadow in store.shadows(None, 20).await? {
        let proposals = shadow
            .evaluation
            .decisions
            .iter()
            .filter(|decision| matches!(decision, Decision::Propose(_)))
            .count();
        shadows.push(json!({
            "id": shadow.id,
            "account": lookup.name(&shadow.ad_account),
            "date": view::timestamp(shadow.evaluation.at),
            "counts": say(text, "opt_shadow_counts", &[
                ("proposals", text.count(proposals as f64)),
                ("holds", text.count((shadow.evaluation.decisions.len() - proposals) as f64)),
            ])?,
        }));
    }
    let shadowing = policies
        .values()
        .any(|current| current.policy.mode == Mode::Shadow);
    let mut activity = Vec::new();
    for event in &events {
        if activity.len() == 8 {
            break;
        }
        let words = event_words(text, event)?;
        if words.is_empty() {
            continue;
        }
        activity.push(json!({
            "text": words,
            "date": view::timestamp(event.at),
            "account": lookup.name(&event.ad_account),
            "proposal": event.proposal_id,
        }));
    }
    format::render().view(
        &v,
        "optimize/index.html",
        json!({
            "open": open, "running": running, "done": done, "accounts": accounts_rows, "activity": activity,
            "shadows": shadows, "shadowing": shadowing,
            "live": settings.optimize.live && settings.optimize.max_mode >= Mode::Recommend,
            "stopped": store.optimization_stopped().await?,
            "access": store.monitoring_access(settings.now().timestamp()).await?,
        }),
    )
}

async fn proposal_page(
    Authenticated(signed): Authenticated,
    State(ctx): State<AppContext>,
    Extension(settings): Extension<Arc<Settings>>,
    ViewEngine(v): ViewEngine<TeraView>,
    Path(id): Path<i64>,
) -> Result<Response> {
    let store = WorkspaceStore::new(&ctx.db, &signed.workspace);
    let Some(proposal) = store.proposal(id).await? else {
        return Ok(crate::app::not_found());
    };
    proposal_view(
        &v,
        signed.user.language.text()?,
        &store,
        &settings,
        &proposal,
        None,
        StatusCode::OK,
    )
    .await
}

async fn proposal_view(
    v: &TeraView,
    text: &'static Text,
    store: &WorkspaceStore<'_>,
    settings: &Settings,
    proposal: &Proposal,
    error: Option<&str>,
    status: StatusCode,
) -> Result<Response> {
    let id = proposal.id;
    let accounts = store.ad_accounts().await?;
    let currency = currency(store, &proposal.ad_account).await?;
    let (current, proposed) = view::values(text, &proposal.draft.action, &currency)?;
    let mut history = Vec::new();
    for event in store
        .optimization_events(Some(&proposal.ad_account), Some(id), 50)
        .await?
    {
        let words = event_words(text, &event)?;
        if !words.is_empty() {
            history.push(json!({"text": words, "date": view::timestamp(event.at)}));
        }
    }
    let execution = store.execution(id).await?;
    let stopped = store.optimization_stopped().await?;
    let policy_mode = store
        .optimization_policy(&proposal.ad_account)
        .await?
        .policy
        .mode;
    let live = installed(settings, &proposal.draft);
    let applicable = live && changes(policy_mode);
    let now = settings.now().timestamp();
    let mut response = format::render().view(
        v,
        "optimize/proposal.html",
        json!({
            "id": proposal.id,
            "account": accounts.iter().find(|ad| ad.customer_id == proposal.ad_account).map(|ad| ad.name.clone()).unwrap_or_default(),
            "account_id": proposal.ad_account,
            "headline": view::headline(text, &proposal.draft, &currency)?,
            "kind": view::kind(text, proposal.draft.action.kind())?,
            "current": current, "proposed": proposed,
            "risk": view::risk(text, proposal.draft.action.risk())?,
            "high": proposal.draft.action.risk() == crate::optimize::Risk::High,
            "reasons": view::reasons(text, &proposal.draft, &currency)?,
            "state": view::state(text, proposal.state)?,
            "open": proposal.state == ProposalState::Open && proposal.draft.expires_at > now,
            "fingerprint": proposal.fingerprint,
            "applicable": applicable && !stopped,
            "not_live": !live && !raises(&proposal.draft.action), "stopped": stopped,
            "raise": raises(&proposal.draft.action),
            "mode_blocks": (!changes(policy_mode)).then(|| say(text, "opt_mode_blocks_notice", &[("mode", mode(text, policy_mode)?)])).transpose()?,
            "expires": view::timestamp(proposal.draft.expires_at),
            "automatic": proposal.draft.approval == crate::optimize::Approval::Automatic,
            "verified": execution.as_ref().is_some_and(|execution| execution.verified),
            "request_id": execution.as_ref().and_then(|execution| execution.request_id.clone()),
            "uncertain": proposal.state == ProposalState::Uncertain,
            "error": error.map(|key| say(text, key, &[])).transpose()?,
            "history": history,
        }),
    )?;
    *response.status_mut() = status;
    Ok(response)
}

#[derive(Deserialize)]
struct Approval {
    fingerprint: String,
}

async fn approve(
    Authenticated(signed): Authenticated,
    State(ctx): State<AppContext>,
    Extension(settings): Extension<Arc<Settings>>,
    Path(id): Path<i64>,
    Form(form): Form<Approval>,
) -> Result<Response> {
    let store = WorkspaceStore::new(&ctx.db, &signed.workspace);
    let Some(proposal) = store.proposal(id).await? else {
        return Ok(crate::app::not_found());
    };
    let now = settings.now().timestamp();
    if !store.monitoring_access(now).await?
        || store.optimization_stopped().await?
        || !installed(&settings, &proposal.draft)
        || !changes(
            store
                .optimization_policy(&proposal.ad_account)
                .await?
                .policy
                .mode,
        )
    {
        return Ok(Redirect::to(&format!("/optimize/proposals/{id}")).into_response());
    }
    if store.approve_proposal(id, &form.fingerprint, now).await? {
        OptimizationWorker::perform_later(
            &ctx,
            OptimizationJob {
                workspace_id: signed.workspace.id().into(),
                task: Task::Execute { proposal_id: id },
            },
        )
        .await?;
    }
    Ok(Redirect::to(&format!("/optimize/proposals/{id}")).into_response())
}

#[derive(Default, Deserialize)]
struct RecoveryForm {
    #[serde(default)]
    fingerprint: String,
    #[serde(default)]
    note: String,
    #[serde(default)]
    confirmed: String,
}

/// The User's report that Google Ads confirmed an Uncertain change was not applied. KeySend
/// calls nothing: it records the report as unverified, and the Ad Account stays in
/// Investigate, suspended, until its settings are saved again.
async fn recover(
    Authenticated(signed): Authenticated,
    State(ctx): State<AppContext>,
    Extension(settings): Extension<Arc<Settings>>,
    ViewEngine(v): ViewEngine<TeraView>,
    Path(id): Path<i64>,
    Form(form): Form<RecoveryForm>,
) -> Result<Response> {
    let store = WorkspaceStore::new(&ctx.db, &signed.workspace);
    let Some(proposal) = store.proposal(id).await? else {
        return Ok(crate::app::not_found());
    };
    let note = form.note.trim();
    if note.is_empty() || note.chars().count() > 1000 || form.confirmed != "yes" {
        return proposal_view(
            &v,
            signed.user.language.text()?,
            &store,
            &settings,
            &proposal,
            Some("opt_recover_error"),
            StatusCode::UNPROCESSABLE_ENTITY,
        )
        .await;
    }
    let report = Report {
        note,
        user_id: &signed.user.id,
    };
    match store
        .recover_uncertain(id, &form.fingerprint, &report, settings.now().timestamp())
        .await?
    {
        Recovery::Recovered | Recovery::Already => {
            Ok(Redirect::to(&format!("/optimize/proposals/{id}")).into_response())
        }
        Recovery::NotUncertain => Ok(StatusCode::CONFLICT.into_response()),
    }
}

#[derive(Default, Deserialize)]
struct Rejection {
    #[serde(default)]
    reason: String,
}

async fn reject(
    Authenticated(signed): Authenticated,
    State(ctx): State<AppContext>,
    Extension(settings): Extension<Arc<Settings>>,
    Path(id): Path<i64>,
    Form(form): Form<Rejection>,
) -> Result<Response> {
    let store = WorkspaceStore::new(&ctx.db, &signed.workspace);
    if store.proposal(id).await?.is_none() {
        return Ok(crate::app::not_found());
    }
    let reason = form.reason.trim();
    if reason.chars().count() > 1000 {
        return Ok(StatusCode::UNPROCESSABLE_ENTITY.into_response());
    }
    store
        .set_proposal_state(
            id,
            ProposalState::Rejected,
            if reason.is_empty() {
                json!({})
            } else {
                json!({"reason": reason})
            },
            settings.now().timestamp(),
        )
        .await?;
    Ok(Redirect::to(&format!("/optimize/proposals/{id}")).into_response())
}

async fn activity(
    Authenticated(signed): Authenticated,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    let store = WorkspaceStore::new(&ctx.db, &signed.workspace);
    let text = signed.user.language.text()?;
    let accounts = store.ad_accounts().await?;
    let lookup = Lookup::load(&store, &accounts).await?;
    let proposals = store
        .proposals(None, &ProposalState::ALL, 500)
        .await?
        .into_iter()
        .map(|proposal| (proposal.id, proposal))
        .collect::<BTreeMap<_, _>>();
    let mut events = Vec::new();
    for event in store.optimization_events(None, None, 200).await? {
        let words = event_words(text, &event)?;
        if words.is_empty() {
            continue;
        }
        let headline = event
            .proposal_id
            .and_then(|id| proposals.get(&id))
            .map(|found| view::headline(text, &found.draft, lookup.currency(&found.ad_account)))
            .transpose()?;
        events.push(json!({
            "text": words, "date": view::timestamp(event.at), "proposal": event.proposal_id, "headline": headline,
            "account": lookup.name(&event.ad_account),
        }));
    }
    format::render().view(&v, "optimize/activity.html", json!({"events": events}))
}

#[derive(Deserialize)]
struct Stop {
    stop: String,
}

async fn stop(
    Authenticated(signed): Authenticated,
    State(ctx): State<AppContext>,
    Extension(settings): Extension<Arc<Settings>>,
    Form(form): Form<Stop>,
) -> Result<Response> {
    WorkspaceStore::new(&ctx.db, &signed.workspace)
        .set_optimization_stopped(form.stop == "stop", settings.now().timestamp())
        .await?;
    Ok(Redirect::to("/optimize").into_response())
}

const FEEDBACK: [(&str, Feedback); 4] = [
    ("useful", Feedback::Useful),
    ("incorrect", Feedback::Incorrect),
    ("too_risky", Feedback::TooRisky),
    ("missing_context", Feedback::MissingContext),
];

fn feedback_name(feedback: &Value) -> Option<&'static str> {
    FEEDBACK
        .iter()
        .map(|(name, _)| *name)
        .find(|name| feedback.as_str() == Some(name))
}

/// The before and after of one proposed change, described and never read as its effect.
fn observed(
    text: &'static Text,
    shadow: &Shadow,
    observation: &Observation,
    currency: &str,
) -> Result<Value> {
    let day = shadow.evaluation.day;
    let window = i64::from(shadow.evaluation.policy.observation_days);
    let values = |spend: f64, conversions: f64| {
        say(
            text,
            "opt_shadow_period_values",
            &[
                ("spend", text.money(spend, currency)),
                ("conversions", text.count(conversions)),
            ],
        )
    };
    let after_end = (day + window).min(observation.data_day);
    let coverage = |found: i64, of: i64| {
        say(
            text,
            "opt_shadow_coverage",
            &[
                ("found", text.count(found as f64)),
                ("window", text.count(of as f64)),
            ],
        )
    };
    Ok(json!({
        "before_days": view::date_range(text, day - window, day - 1),
        "before": values(observation.before_spend, observation.before_conversions)?,
        "before_coverage": coverage(observation.before_days, window)?,
        "after_days": (observation.settled_days > 0 && after_end > day).then(|| view::date_range(text, day + 1, after_end)),
        "after": values(observation.after_spend, observation.after_conversions)?,
        "after_coverage": coverage(observation.settled_days, after_end - day)?,
        "partial": observation.before_days < window || observation.settled_days < window,
        "unreliable": !observation.tracking_reliable,
        "external": (observation.external_changes > 0).then(|| say(text, "opt_shadow_external", &[("changes", text.count(observation.external_changes as f64))])).transpose()?,
    }))
}

#[derive(Default)]
struct ShadowSelection {
    chosen: Option<usize>,
    error: Option<usize>,
}

async fn shadow_page(
    v: &TeraView,
    text: &'static Text,
    store: &WorkspaceStore<'_>,
    settings: &Settings,
    shadow: &Shadow,
    selection: ShadowSelection,
    status: StatusCode,
) -> Result<Response> {
    let ShadowSelection { chosen, error } = selection;
    let evaluation = &shadow.evaluation;
    let now = settings.now().timestamp();
    let currency = currency(store, &shadow.ad_account).await?;
    let events = store.shadow_events(shadow.id).await?;
    let observations = store
        .latest_shadow_observation(shadow.id)
        .await?
        .as_ref()
        .and_then(|event| {
            serde_json::from_value::<Vec<Observation>>(event.detail["observations"].clone()).ok()
        })
        .unwrap_or_default();
    let mut decisions = Vec::new();
    for (item, decision) in evaluation.decisions.iter().enumerate() {
        let mut feedback = Vec::new();
        for event in events.iter().filter(|event| {
            event.kind == "feedback" && event.detail["item"].as_u64() == Some(item as u64)
        }) {
            if let Some(name) = feedback_name(&event.detail["feedback"]) {
                feedback.push(json!({
                    "text": say(text, "opt_shadow_feedback_given", &[("feedback", say(text, &format!("opt_feedback_{name}"), &[])?)])?,
                    "note": event.detail["note"].as_str().unwrap_or_default(),
                    "date": view::timestamp(event.at),
                }));
            }
        }
        let mut found = match decision {
            Decision::Propose(draft) => {
                let (current, proposed) = view::values(text, &draft.action, &currency)?;
                json!({
                    "proposed": true,
                    "headline": view::headline(text, draft, &currency)?,
                    "kind": view::kind(text, draft.action.kind())?,
                    "risk": view::risk(text, draft.action.risk())?,
                    "high": draft.action.risk() == crate::optimize::Risk::High,
                    "current": current, "proposed_value": proposed,
                    "reasons": view::reasons(text, draft, &currency)?,
                    "rules": [say(text, if draft.approval == crate::optimize::Approval::Automatic { "opt_shadow_automatic" } else { "opt_shadow_needs_approval" }, &[])?],
                    "expires": say(text, "opt_shadow_expires", &[("date", view::timestamp(draft.expires_at))])?,
                    "observed": observations
                        .iter()
                        .find(|observation| observation.campaign_id == draft.action.campaign_id())
                        .map(|observation| observed(text, shadow, observation, &currency))
                        .transpose()?,
                })
            }
            Decision::Hold(signal, hold) => json!({
                "proposed": false,
                "headline": view::held(text, signal)?,
                "facts": view::fact(text, signal, &currency)?,
                "rules": [view::hypothesis(text, signal.kind)?, view::hold(text, hold.name())?],
            }),
        };
        found["item"] = json!(item);
        found["feedback"] = json!(feedback);
        found["error"] = json!(error == Some(item));
        found["open"] = json!(match error.or(chosen) {
            Some(shown) => shown == item,
            None =>
                evaluation
                    .decisions
                    .iter()
                    .position(|decision| matches!(decision, Decision::Propose(_)))
                    == Some(item),
        });
        decisions.push(found);
    }
    let latest = store
        .shadows(Some(&shadow.ad_account), 1)
        .await?
        .first()
        .map(|newest| newest.evaluation.data_day)
        .into_iter()
        .chain(observations.iter().map(|observation| observation.data_day))
        .max()
        .unwrap_or(evaluation.data_day);
    let version = store.optimization_policy(&shadow.ad_account).await?.version;
    let mut stale = Vec::new();
    if let Some(expiry) = evaluation
        .decisions
        .iter()
        .filter_map(|decision| match decision {
            Decision::Propose(draft) => Some(draft.expires_at),
            Decision::Hold(..) => None,
        })
        .min()
        .filter(|expiry| *expiry <= now)
    {
        stale.push(say(
            text,
            "opt_shadow_stale_expired",
            &[("date", view::timestamp(expiry))],
        )?);
    }
    if latest > evaluation.data_day {
        stale.push(say(
            text,
            "opt_shadow_stale_data",
            &[("date", view::date(text, latest))],
        )?);
    }
    if version != evaluation.policy_version {
        stale.push(say(
            text,
            "opt_shadow_stale_policy",
            &[("version", version.to_string())],
        )?);
    }
    let snapshot = &evaluation.snapshot;
    let days = snapshot["days"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let first = days.iter().filter_map(|day| day["day"].as_i64()).min();
    let count = |key: &str| text.count(snapshot[key].as_array().map_or(0, Vec::len) as f64);
    let accounts = store.ad_accounts().await?;
    let mut response = format::render().view(
        v,
        "optimize/shadow.html",
        json!({
            "id": shadow.id,
            "account": accounts.iter().find(|ad| ad.customer_id == shadow.ad_account).map(|ad| ad.name.clone()).unwrap_or_default(),
            "account_id": shadow.ad_account,
            "evaluated": view::timestamp(evaluation.at),
            "version": evaluation.policy_version,
            "mode": mode(text, evaluation.policy.mode)?,
            "data_day": view::date(text, evaluation.data_day),
            "period": first.map(|first| view::date_range(text, first, evaluation.data_day)),
            "campaigns": count("campaigns"), "changes": count("changes"), "findings": count("findings"),
            "tracking_reliable": snapshot["tracking_reliable"].as_bool().unwrap_or(false),
            "stale": stale,
            "decisions": decisions,
            "abstention": say(text, match evaluation.abstention {
                Some(Abstention::MissingCommercialTarget) => "opt_shadow_no_target",
                Some(Abstention::NoSignal) | None => "opt_shadow_empty",
            }, &[])?,
            "feedback": FEEDBACK.iter().map(|(name, _)| Ok(json!({"name": name, "label": say(text, &format!("opt_feedback_{name}"), &[])?}))).collect::<Result<Vec<_>>>()?,
        }),
    )?;
    *response.status_mut() = status;
    Ok(response)
}

/// The decision that the page opens, such as the one the User just rated.
#[derive(Default, Deserialize)]
struct Shown {
    #[serde(default)]
    decision: String,
}

async fn shadow(
    Authenticated(signed): Authenticated,
    State(ctx): State<AppContext>,
    Extension(settings): Extension<Arc<Settings>>,
    ViewEngine(v): ViewEngine<TeraView>,
    Path(id): Path<i64>,
    Query(shown): Query<Shown>,
) -> Result<Response> {
    let store = WorkspaceStore::new(&ctx.db, &signed.workspace);
    let Some(shadow) = store.shadow(id).await? else {
        return Ok(crate::app::not_found());
    };
    let chosen = shown
        .decision
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|item| *item < shadow.evaluation.decisions.len());
    shadow_page(
        &v,
        signed.user.language.text()?,
        &store,
        &settings,
        &shadow,
        ShadowSelection {
            chosen,
            ..Default::default()
        },
        StatusCode::OK,
    )
    .await
}

#[derive(Deserialize)]
struct FeedbackForm {
    #[serde(default)]
    item: String,
    #[serde(default)]
    feedback: String,
    #[serde(default)]
    note: String,
}

async fn shadow_feedback(
    Authenticated(signed): Authenticated,
    State(ctx): State<AppContext>,
    Extension(settings): Extension<Arc<Settings>>,
    ViewEngine(v): ViewEngine<TeraView>,
    Path(id): Path<i64>,
    Form(form): Form<FeedbackForm>,
) -> Result<Response> {
    let store = WorkspaceStore::new(&ctx.db, &signed.workspace);
    let Some(shadow) = store.shadow(id).await? else {
        return Ok(crate::app::not_found());
    };
    let item = form
        .item
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|item| *item < shadow.evaluation.decisions.len());
    let feedback = FEEDBACK
        .iter()
        .find(|(name, _)| *name == form.feedback)
        .map(|(_, feedback)| feedback.clone());
    let note = form.note.trim();
    let Some(item) = item else {
        return Ok(StatusCode::UNPROCESSABLE_ENTITY.into_response());
    };
    let Some(feedback) = feedback.filter(|_| note.chars().count() <= 1000) else {
        return shadow_page(
            &v,
            signed.user.language.text()?,
            &store,
            &settings,
            &shadow,
            ShadowSelection {
                error: Some(item),
                ..Default::default()
            },
            StatusCode::UNPROCESSABLE_ENTITY,
        )
        .await;
    };
    store
        .record_shadow_feedback(id, item, feedback, note, settings.now().timestamp())
        .await?;
    Ok(Redirect::to(&format!(
        "/optimize/shadows/{id}?decision={item}#decision-{item}"
    ))
    .into_response())
}

/// The form of the Optimization policy. Amounts are in the Ad Account currency.
#[derive(Default, Deserialize)]
struct PolicyForm {
    #[serde(default)]
    mode: String,
    #[serde(default)]
    automatic: Vec<String>,
    #[serde(default)]
    protected: Vec<i64>,
    #[serde(default)]
    max_decrease_percent: String,
    #[serde(default)]
    max_increase_percent: String,
    #[serde(default)]
    envelope: String,
    #[serde(default)]
    max_daily_increase: String,
    #[serde(default)]
    changes_per_day: String,
    #[serde(default)]
    cooldown_days: String,
    #[serde(default)]
    min_conversions: String,
    #[serde(default)]
    target_cost: String,
    #[serde(default)]
    shadow_consent: String,
}

fn micros(value: &str) -> Option<Option<i64>> {
    let value = value.trim();
    if value.is_empty() {
        return Some(None);
    }
    value
        .replace(',', ".")
        .parse::<f64>()
        .ok()
        .filter(|amount| amount.is_finite() && *amount > 0.0 && *amount < 1e9)
        .map(|amount| Some((amount * 1e6).round() as i64))
}

impl PolicyForm {
    fn policy(&self, base: &Policy, campaigns: &BTreeSet<i64>) -> Option<Policy> {
        let number = |value: &str| value.trim().parse::<u32>().ok();
        let policy = Policy {
            mode: match self.mode.as_str() {
                "investigate" => Mode::Investigate,
                "shadow" => Mode::Shadow,
                "recommend" => Mode::Recommend,
                "autopilot" => Mode::Autopilot,
                _ => return None,
            },
            automatic: self
                .automatic
                .iter()
                .map(|kind| ActionKind::from_name(kind))
                .collect::<Option<_>>()?,
            protected: self
                .protected
                .iter()
                .map(|campaign| campaigns.contains(campaign).then_some(*campaign))
                .collect::<Option<_>>()?,
            max_decrease_percent: number(&self.max_decrease_percent)?,
            max_increase_percent: number(&self.max_increase_percent)?,
            envelope_micros: micros(&self.envelope)?,
            max_daily_increase_micros: micros(&self.max_daily_increase)?.unwrap_or(0),
            changes_per_day: number(&self.changes_per_day)?,
            cooldown_days: number(&self.cooldown_days)?,
            min_conversions: number(&self.min_conversions)?,
            target_cost_micros: micros(&self.target_cost)?,
            ..base.clone()
        };
        policy.valid().then_some(policy)
    }
}

async fn policy_page(
    v: &TeraView,
    text: &'static Text,
    store: &WorkspaceStore<'_>,
    settings: &Settings,
    ad: &AdAccount,
    error: Option<&str>,
    status: StatusCode,
) -> Result<Response> {
    let history = store
        .optimization_policy_history(&ad.customer_id, 20)
        .await?;
    let current = store.optimization_policy(&ad.customer_id).await?;
    let money = |micros: Option<i64>| {
        micros.map_or_else(String::new, |micros| format!("{:.2}", micros as f64 / 1e6))
    };
    let campaigns = store
        .campaigns(&ad.customer_id)
        .await?
        .into_iter()
        .map(|campaign| json!({"id": campaign.id, "name": campaign.name, "protected": current.policy.protected.contains(&campaign.id)}))
        .collect::<Vec<_>>();
    let automatic = policy::AUTOMATIC_KINDS
        .into_iter()
        .map(|kind| {
            Ok(json!({
                "name": kind.name(),
                "label": view::kind(text, kind)?,
                "checked": current.policy.automatic.contains(&kind),
                "live": settings.optimize.live && settings.optimize.actions.contains(&kind),
            }))
        })
        .collect::<Result<Vec<_>>>()?;
    let versions = history
        .iter()
        .map(|version| {
            Ok(json!({
                "version": version.version,
                "mode": mode(text, version.policy.mode)?,
                "standing": standing(text, version.standing, &version.reason)?,
                "date": view::timestamp(version.created_at),
            }))
        })
        .collect::<Result<Vec<_>>>()?;
    let policy = &current.policy;
    let mut response = format::render().view(
        v,
        "optimize/account.html",
        json!({
            "id": ad.customer_id, "name": ad.name, "currency": currency(store, &ad.customer_id).await?,
            "mode": match policy.mode { Mode::Investigate => "investigate", Mode::Shadow => "shadow", Mode::Recommend => "recommend", Mode::Autopilot => "autopilot" },
            "standing": standing(text, current.standing, &current.reason)?,
            "active": current.standing == Standing::Active,
            "autopilot": policy.mode == Mode::Autopilot,
            "automatic": automatic, "campaigns": campaigns,
            "max_decrease_percent": policy.max_decrease_percent, "max_increase_percent": policy.max_increase_percent,
            "envelope": money(policy.envelope_micros), "max_daily_increase": money(Some(policy.max_daily_increase_micros).filter(|micros| *micros > 0)),
            "changes_per_day": policy.changes_per_day, "cooldown_days": policy.cooldown_days, "min_conversions": policy.min_conversions,
            "target_cost": money(policy.target_cost_micros),
            "limits": {"increase": policy::MAX_INCREASE_PERCENT, "decrease": policy::MAX_DECREASE_PERCENT, "changes": policy::MAX_CHANGES_PER_DAY},
            "versions": versions, "live": settings.optimize.live,
            "available": {
                "shadow": Mode::Shadow <= settings.optimize.max_mode || policy.mode == Mode::Shadow,
                "recommend": Mode::Recommend <= settings.optimize.max_mode || policy.mode == Mode::Recommend,
                "autopilot": Mode::Autopilot <= settings.optimize.max_mode || policy.mode == Mode::Autopilot,
            },
            "ceiling": (policy.mode > settings.optimize.max_mode).then(|| say(text, "opt_mode_ceiling_notice", &[("mode", mode(text, policy.mode)?)])).transpose()?,
            "error": error.map(|key| say(text, key, &[])).transpose()?,
        }),
    )?;
    *response.status_mut() = status;
    Ok(response)
}

async fn account(
    OwnedAdAccount { signed, ad }: OwnedAdAccount,
    State(ctx): State<AppContext>,
    Extension(settings): Extension<Arc<Settings>>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    let store = WorkspaceStore::new(&ctx.db, &signed.workspace);
    policy_page(
        &v,
        signed.user.language.text()?,
        &store,
        &settings,
        &ad,
        None,
        StatusCode::OK,
    )
    .await
}

async fn save(
    OwnedAdAccount { signed, ad }: OwnedAdAccount,
    State(ctx): State<AppContext>,
    Extension(settings): Extension<Arc<Settings>>,
    ViewEngine(v): ViewEngine<TeraView>,
    Form(form): Form<PolicyForm>,
) -> Result<Response> {
    let store = WorkspaceStore::new(&ctx.db, &signed.workspace);
    let current = store.optimization_policy(&ad.customer_id).await?;
    let campaigns = store
        .campaigns(&ad.customer_id)
        .await?
        .into_iter()
        .map(|campaign| campaign.id)
        .collect();
    // Shadow records the Ad Account each day, so it starts only with the User's explicit consent.
    let error = match form.policy(&current.policy, &campaigns) {
        None => Err("opt_policy_error"),
        // The installation's ceiling refuses a higher mode; a stored one stays as it is.
        Some(policy)
            if policy.mode > settings.optimize.max_mode && policy.mode != current.policy.mode =>
        {
            Err("opt_mode_unavailable")
        }
        Some(policy)
            if policy.mode == Mode::Shadow
                && current.policy.mode != Mode::Shadow
                && form.shadow_consent != "yes" =>
        {
            Err("opt_shadow_consent_error")
        }
        Some(policy) => Ok(policy),
    };
    let policy = match error {
        Ok(policy) => policy,
        Err(key) => {
            return policy_page(
                &v,
                signed.user.language.text()?,
                &store,
                &settings,
                &ad,
                Some(key),
                StatusCode::UNPROCESSABLE_ENTITY,
            )
            .await;
        }
    };
    let now = settings.now().timestamp();
    // Saving the settings never resumes a suspended or switched-off Autopilot; only its own
    // control does.
    let (standing, reason) = match policy.mode {
        Mode::Autopilot if current.version > 0 => (current.standing, current.reason.clone()),
        Mode::Shadow => (Standing::Active, "shadow_consent".to_owned()),
        _ => (Standing::Active, "user".to_owned()),
    };
    store
        .save_optimization_policy(&ad.customer_id, &policy, standing, &reason, now)
        .await?;
    if policy.mode != Mode::Investigate {
        OptimizationWorker::perform_later(
            &ctx,
            OptimizationJob {
                workspace_id: signed.workspace.id().into(),
                task: Task::Evaluate {
                    ad_account: ad.customer_id.clone(),
                },
            },
        )
        .await?;
    }
    Ok(Redirect::to(&format!("/ad-accounts/{}/optimize", ad.customer_id)).into_response())
}

/// The Autopilot of one Ad Account: switch it off (revoked), or resume it after a suspension.
async fn autopilot(
    OwnedAdAccount { signed, ad }: OwnedAdAccount,
    State(ctx): State<AppContext>,
    Extension(settings): Extension<Arc<Settings>>,
    Form(form): Form<Stop>,
) -> Result<Response> {
    let store = WorkspaceStore::new(&ctx.db, &signed.workspace);
    let current = store.optimization_policy(&ad.customer_id).await?;
    let next = if form.stop == "stop" {
        Standing::Revoked
    } else {
        Standing::Active
    };
    if current.version > 0 && current.standing != next {
        store
            .save_optimization_policy(
                &ad.customer_id,
                &current.policy,
                next,
                "user",
                settings.now().timestamp(),
            )
            .await?;
    }
    Ok(Redirect::to(&format!("/ad-accounts/{}/optimize", ad.customer_id)).into_response())
}

pub fn routes() -> Routes {
    Routes::new()
        .add("/optimize", get(index))
        .add("/optimize/activity", get(activity))
        .add("/optimize/stop", post(stop))
        .add("/optimize/proposals/{id}", get(proposal_page))
        .add("/optimize/proposals/{id}/approve", post(approve))
        .add("/optimize/proposals/{id}/reject", post(reject))
        .add("/optimize/proposals/{id}/recover", post(recover))
        .add("/optimize/shadows/{id}", get(shadow))
        .add("/optimize/shadows/{id}/feedback", post(shadow_feedback))
        .add("/ad-accounts/{id}/optimize", get(account).post(save))
        .add("/ad-accounts/{id}/optimize/autopilot", post(autopilot))
}
