use std::{collections::BTreeMap, sync::Arc};

use axum::{
    extract::{Path, Query},
    http::StatusCode,
    response::{IntoResponse, Redirect},
    Extension,
};
use axum_extra::extract::Form;
use loco_rs::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{account_path, campaign_path, clock};
use crate::{
    app::not_found,
    auth::OwnedAdAccount,
    language::Text,
    monitoring::{
        campaign_settings::{CampaignSettings, Choice, NOTE_LENGTH},
        tracking::ConversionTracking,
    },
    settings::Settings,
    workspace::{history::Campaign, AdAccount, WorkspaceStore},
};

pub(super) fn path(id: &str, campaign: i64) -> String {
    format!("{}/settings", campaign_path(id, campaign))
}

#[derive(Serialize)]
struct Values {
    target_cost: String,
    note: String,
    alerts: &'static str,
    judge: &'static str,
}

impl Values {
    fn of(settings: &CampaignSettings) -> Self {
        Self {
            target_cost: settings
                .target_cost
                .map(|cost| format!("{cost:.2}"))
                .unwrap_or_default(),
            note: settings.note.clone().unwrap_or_default(),
            alerts: settings.alerts.word("on", "off"),
            judge: settings.judge_by_conversions.word("yes", "no"),
        }
    }
}

#[derive(Serialize)]
struct FieldError {
    key: &'static str,
    field: &'static str,
    message: &'static str,
}

#[derive(Deserialize)]
pub(super) struct SavedQuery {
    saved: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Submitted {
    target_cost: String,
    note: String,
    alerts: String,
    judge_by_conversions: String,
}

async fn open(
    store: &WorkspaceStore<'_>,
    settings: &Settings,
    id: &str,
    campaign_id: i64,
) -> Result<std::result::Result<Campaign, Response>> {
    let Some(campaign) = store
        .campaigns(id)
        .await?
        .into_iter()
        .find(|campaign| campaign.id == campaign_id)
    else {
        return Ok(Err(not_found()));
    };
    if !store.monitoring_access(settings.now().timestamp()).await? {
        return Ok(Err(Redirect::to(&account_path(id)).into_response()));
    }
    Ok(Ok(campaign))
}

#[allow(clippy::too_many_arguments)]
async fn page(
    text: &Text,
    v: &TeraView,
    store: &WorkspaceStore<'_>,
    settings: &Settings,
    ad: &AdAccount,
    campaign: &Campaign,
    submitted: Option<(Values, Vec<FieldError>)>,
    saved: bool,
) -> Result<Response> {
    let id = &ad.customer_id;
    let clock = clock(store, settings, id).await?;
    let rules = store.resolved_settings(id, clock.today - 1).await?;
    let account = rules.account();
    let refused = submitted.is_some();
    let (values, errors) = submitted.unwrap_or_else(|| {
        (
            Values::of(&rules.own(campaign.id).cloned().unwrap_or_default()),
            Vec::new(),
        )
    });
    let currency = clock.currency.unwrap_or_default();
    let mut response = format::render().view(
        v,
        "monitoring/campaign_settings.html",
        json!({
            "ad": {"id": id, "name": ad.name},
            "campaign": {"id": campaign.id, "name": campaign.name, "href": campaign_path(id, campaign.id)},
            "action": path(id, campaign.id),
            "values": values,
            "field_errors": errors.iter().map(|error| (error.key, error.message)).collect::<BTreeMap<_, _>>(),
            "errors": errors,
            "saved": saved,
            "account": {
                "target": account.target_cost.filter(|cost| *cost > 0.0).map(|cost| text.money(cost, &currency)),
                "alerts": account.alerts,
                "judged": account.judged,
            },
            "tracking": match account.state {
                ConversionTracking::Working => "working",
                ConversionTracking::TooFewResults => "too_few_results",
                ConversionTracking::NotSetUp => "not_set_up",
            },
            "yes_has_no_effect": !account.judged && account.state != ConversionTracking::Working,
            "alerts_off": !rules.effective(campaign.id).alerts,
            "currency_mark": text.written.format.currency_mark(&currency),
            "note_length": NOTE_LENGTH,
        }),
    )?;
    if refused {
        *response.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
    }
    Ok(response)
}

pub(super) async fn show(
    OwnedAdAccount { signed, ad }: OwnedAdAccount,
    State(ctx): State<AppContext>,
    Extension(settings): Extension<Arc<Settings>>,
    ViewEngine(v): ViewEngine<TeraView>,
    Path((id, campaign_id)): Path<(String, i64)>,
    Query(query): Query<SavedQuery>,
) -> Result<Response> {
    let store = WorkspaceStore::new(&ctx.db, &signed.workspace);
    let campaign = match open(&store, &settings, &id, campaign_id).await? {
        Ok(found) => found,
        Err(refused) => return Ok(refused),
    };
    page(
        signed.user.language.text()?,
        &v,
        &store,
        &settings,
        &ad,
        &campaign,
        None,
        query.saved.is_some(),
    )
    .await
}

pub(super) async fn save(
    OwnedAdAccount { signed, ad }: OwnedAdAccount,
    State(ctx): State<AppContext>,
    Extension(settings): Extension<Arc<Settings>>,
    ViewEngine(v): ViewEngine<TeraView>,
    Path((id, campaign_id)): Path<(String, i64)>,
    Form(form): Form<Submitted>,
) -> Result<Response> {
    let text = signed.user.language.text()?;
    let store = WorkspaceStore::new(&ctx.db, &signed.workspace);
    let campaign = match open(&store, &settings, &id, campaign_id).await? {
        Ok(found) => found,
        Err(refused) => return Ok(refused),
    };
    let (Some(alerts), Some(judge_by_conversions)) = (
        Choice::parse(&form.alerts, "on", "off"),
        Choice::parse(&form.judge_by_conversions, "yes", "no"),
    ) else {
        return Ok(StatusCode::UNPROCESSABLE_ENTITY.into_response());
    };
    let mut errors = Vec::new();
    let target_cost = super::TargetCost::parse(&form.target_cost);
    if target_cost == super::TargetCost::NotANumber {
        errors.push(FieldError {
            key: "target",
            field: "target",
            message: text.written.page_target_not_number_or_empty,
        });
    }
    let target_cost = target_cost.amount();
    let note = form.note.trim();
    let new = CampaignSettings {
        target_cost,
        note: (!note.is_empty()).then(|| note.to_owned()),
        alerts,
        judge_by_conversions,
    };
    if target_cost.is_some_and(|cost| !(cost > 0.0 && cost < 1e9)) {
        errors.push(FieldError {
            key: "target",
            field: "target",
            message: text.written.page_target_above_zero,
        });
    }
    if note.chars().count() > NOTE_LENGTH {
        errors.push(FieldError {
            key: "note",
            field: "note",
            message: text.written.page_note_length,
        });
    }
    if !errors.is_empty() {
        let values = Values {
            target_cost: form.target_cost,
            note: form.note,
            alerts: alerts.word("on", "off"),
            judge: judge_by_conversions.word("yes", "no"),
        };
        return page(
            text,
            &v,
            &store,
            &settings,
            &ad,
            &campaign,
            Some((values, errors)),
            false,
        )
        .await;
    }
    if !store.save_campaign_settings(&id, campaign.id, &new).await? {
        return Ok(not_found());
    }
    Ok(Redirect::to(&format!("{}?saved=1", path(&id, campaign.id))).into_response())
}
