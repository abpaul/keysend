use std::{collections::BTreeMap, sync::Arc};

use axum::{
    extract::{DefaultBodyLimit, Multipart, Path, Query},
    http::StatusCode,
    response::{IntoResponse, Redirect},
    Extension,
};
use axum_extra::extract::Form;
use loco_rs::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    app::not_found,
    auth::{Authenticated, OwnedAdAccount},
    identity::SignedIn,
    language::Text,
    monitoring::{
        access,
        branding::compress_logo,
        campaign_text::Campaigns,
        causes,
        charts::{TrendMetric, HISTORY_DAYS},
        explain, follow_up,
        metrics::{self, CampaignDay, Window},
        tracking::{self, ConversionTracking},
        update::Update,
        view, UPDATES_SHOWN,
    },
    settings::Settings,
    translation::{Translations, Wanted},
    workspace::{
        history::{day_number, local_time, Campaign, HistoryState},
        monitoring::{
            AccountSettings, EarlierUpdate, RecommendationState, RecommendationWithFinding,
        },
        profile::Profile,
        settings::AdAccountSettings,
        AdAccount, WorkspaceStore,
    },
};

mod breakdown_pages;
mod campaign_settings;
mod chart_pages;
pub mod example;
mod print_charts;
pub use chart_pages::RawChange;
pub use print_charts::{
    campaign_deep_check, campaign_export_charts, campaign_sparklines, export_changes,
    export_charts, first_read_day, metric_sparklines, Collected,
};

const TRACKING_DAYS: i64 = 30;
const BRANDING_BODY_LIMIT: usize = 6 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum TargetCost {
    Empty,
    Amount(f64),
    NotANumber,
}

impl TargetCost {
    pub(super) fn parse(text: &str) -> Self {
        match text.trim() {
            "" => Self::Empty,
            text => match text.parse::<f64>() {
                Ok(cost) if cost.is_finite() => Self::Amount(cost),
                _ => Self::NotANumber,
            },
        }
    }

    pub(super) fn amount(self) -> Option<f64> {
        match self {
            Self::Amount(cost) => Some(cost),
            Self::Empty | Self::NotANumber => None,
        }
    }
}

fn judgement_override(choice: bool, tracking_works: bool) -> Option<bool> {
    (choice != tracking_works).then_some(choice)
}

async fn compressed_logo(bytes: axum::body::Bytes) -> Result<Option<Vec<u8>>> {
    Ok(tokio::task::spawn_blocking(move || compress_logo(&bytes))
        .await
        .map_err(|_| Error::Message("Logo processing failed".into()))?
        .ok())
}

#[derive(Deserialize)]
struct WindowQuery {
    window: Option<String>,
    metric: Option<String>,
    recommendations: Option<String>,
}

const PANEL_OPEN: &str = "open";

fn window_of(text: Option<&str>) -> Window {
    if text == Some("30") {
        Window::ThirtyDays
    } else {
        Window::SevenDays
    }
}

fn with_window(path: &str, window: Window) -> String {
    match window {
        Window::SevenDays => path.to_owned(),
        Window::ThirtyDays => format!("{path}?window=30"),
    }
}

fn account_path(id: &str) -> String {
    format!("/ad-accounts/{id}")
}

fn campaign_path(id: &str, campaign: i64) -> String {
    format!("/ad-accounts/{id}/campaigns/{campaign}")
}

struct Clock {
    today: i64,
    currency: Option<String>,
}

async fn clock(store: &WorkspaceStore<'_>, settings: &Settings, id: &str) -> Result<Clock> {
    let now = settings.now();
    Ok(match store.ad_account_metadata(id).await? {
        Some((currency, zone)) => Clock {
            today: day_number(local_time(now, &zone)?.date_naive()),
            currency: Some(currency),
        },
        None => Clock {
            today: day_number(now.date_naive()),
            currency: None,
        },
    })
}

struct Numbers {
    text: &'static Text,
    currency: String,
    from: i64,
    end: i64,
    first_day: i64,
    state: ConversionTracking,
    monitoring: AccountSettings,
    judged: bool,
    notice: Option<view::Notice>,
    days: Vec<CampaignDay>,
}

impl Numbers {
    fn source<'a>(&'a self, id: &'a str, window: Window) -> chart_pages::Source<'a> {
        chart_pages::Source {
            text: self.text,
            id,
            days: &self.days,
            from: self.from,
            end: self.end,
            first_day: self.first_day,
            judged: self.judged,
            currency: &self.currency,
            window,
        }
    }
}

async fn numbers(
    text: &'static Text,
    store: &WorkspaceStore<'_>,
    id: &str,
    clock: &Clock,
    history: Option<&HistoryState>,
    days: i64,
) -> Result<Option<Numbers>> {
    let Some(currency) = clock.currency.clone() else {
        return Ok(None);
    };
    let Some((first, last)) = history.and_then(|state| state.first_day.zip(state.last_day)) else {
        return Ok(None);
    };
    let end = metrics::last_shown_day(day_number(last), clock.today);
    let monitoring = store.monitoring_settings(id).await?;
    let chosen = monitoring.judge_by_conversions;
    let from = end - days.max(TRACKING_DAYS) + 1;
    let days = store.monitoring_days(id, from, end).await?;
    let (state, judged) =
        tracking::of_days(&days, end, store.conversion_actions(id).await?, chosen);
    Ok(Some(Numbers {
        text,
        currency,
        from,
        end,
        first_day: day_number(first),
        state,
        monitoring,
        judged,
        notice: view::tracking_notice(text, state, chosen, judged),
        days,
    }))
}

fn open_recommendations(
    all: Vec<RecommendationWithFinding>,
    campaign: Option<i64>,
    campaigns: &Campaigns,
) -> Vec<Value> {
    let mut open = all
        .into_iter()
        .filter(|found| {
            found.recommendation.state == RecommendationState::Open
                && campaign.is_none_or(|id| found.recommendation.campaign_id == Some(id))
        })
        .collect::<Vec<_>>();
    let spend = |found: &RecommendationWithFinding| {
        found
            .finding
            .as_ref()
            .map_or(f64::NEG_INFINITY, |finding| finding.spend)
    };
    open.sort_by(|a, b| spend(b).total_cmp(&spend(a)));
    open.into_iter()
        .map(
            |RecommendationWithFinding {
                 recommendation,
                 finding,
             }| {
                let about = recommendation.campaign_id;
                let action = campaigns.line(&recommendation.action);
                json!({
                    "finding_key": recommendation.finding_key,
                    "subject": recommendation.subject,
                    "confidence": recommendation.confidence,
                    "action": action.text,
                    "action_english": action.english,
                    "action_parts": campaigns.parts(&recommendation.action, about),
                    "finding": finding.map(|finding| campaigns.parts(&finding.title, about)),
                    "impact": campaigns.parts(&recommendation.reason, about),
                    "measure": if recommendation.measure.trim().is_empty() { None } else { Some(campaigns.parts(&recommendation.measure, about)) },
                    "evidence": campaigns.parts(&recommendation.evidence, about),
                })
            },
        )
        .collect()
}

fn panel_href(path: &str, window: Window) -> String {
    let base = with_window(path, window);
    let join = if base.contains('?') { '&' } else { '?' };
    format!("{base}{join}recommendations={PANEL_OPEN}#recommendations")
}

fn earlier(text: &Text, updates: &[EarlierUpdate], campaigns: &Campaigns) -> Result<Vec<Value>> {
    view::earlier(text, updates, |update| update.day, |update| update.quiet)?
        .into_iter()
        .map(|item| {
            Ok(match item {
                view::Earlier::One(update) => json!({
                    "date": view::long_date(text, update.day)?,
                    "summary": campaigns.parts(&update.summary, None),
                }),
                view::Earlier::Quiet(date) => json!({"date": date, "quiet": true}),
            })
        })
        .collect()
}

fn hero(
    text: &Text,
    update: &Update,
    today: i64,
    names: &BTreeMap<i64, String>,
    campaigns: &Campaigns,
) -> Result<Value> {
    let lines = update
        .lines
        .iter()
        .map(|line| {
            let name = update
                .detection
                .campaigns
                .iter()
                .find(|campaign| campaign.campaign_id == line.campaign_id)
                .map(|campaign| campaign.name.as_str())
                .or_else(|| names.get(&line.campaign_id).map(String::as_str))
                .ok_or_else(|| Error::Message("Update names an unknown Campaign".into()))?;
            let shown = campaigns.line(&line.text);
            Ok(json!({
                "name": name,
                "href": campaigns.href(line.campaign_id),
                "text": shown.text,
                "english": shown.english,
                "cause": line
                    .cause
                    .as_ref()
                    .map(|cause| causes::view(text, cause, campaigns, Some(line.campaign_id))),
            }))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(json!({
        "today": update.day == today,
        "date": view::long_date(text, update.day)?,
        "day": view::iso_date(update.day)?,
        "summary": campaigns.summary(update),
        "cause": update
            .cause
            .as_ref()
            .map(|cause| causes::view(text, cause, campaigns, None)),
        "lines": lines,
        "more_campaigns": update.more_campaigns,
        "quiet": update.detection.quiet(),
        "not_useful": update.not_useful,
    }))
}

async fn add_options(store: &WorkspaceStore<'_>, settings: &Settings) -> Result<Value> {
    let used = store.ad_accounts().await?.len();
    let can_add = used < access::AD_ACCOUNT_LIMIT
        && store
            .monitoring_can_select_accounts(&[], settings.now().timestamp())
            .await?;
    Ok(json!({
        "used": used,
        "limit": access::AD_ACCOUNT_LIMIT,
        "can_add": can_add,
        "google_connections": store.google_connections().await?,
    }))
}

async fn index(
    Authenticated(signed): Authenticated,
    State(ctx): State<AppContext>,
    Extension(settings): Extension<Arc<Settings>>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    let text = signed.user.language.text()?;
    let store = WorkspaceStore::new(&ctx.db, &signed.workspace);
    let accounts = store.ad_accounts().await?;
    if let [only] = accounts.as_slice() {
        return Ok(Redirect::to(&account_path(&only.customer_id)).into_response());
    }
    let summaries = store.latest_summaries().await?;
    let mut wanted = Wanted::default();
    for (_, _, summary) in &summaries {
        wanted.text(summary);
    }
    let translations = Translations::load(&store, signed.user.language, wanted).await?;
    let cards = accounts
        .iter()
        .map(|ad| {
            let update = summaries
                .iter()
                .find(|(id, _, _)| *id == ad.customer_id)
                .map(|(_, day, summary)| {
                    let shown = translations.shown(summary);
                    Ok::<_, Error>(json!({
                        "sentence": view::first_sentence(shown.text),
                        "english": shown.english,
                        "date": view::long_date(text, *day)?,
                    }))
                })
                .transpose()?;
            Ok(json!({"name": ad.name, "href": account_path(&ad.customer_id), "update": update}))
        })
        .collect::<Result<Vec<_>>>()?;
    format::render().view(
        &v,
        "monitoring/ad_accounts.html",
        json!({"accounts": cards, "add": add_options(&store, &settings).await?}),
    )
}

async fn show(
    OwnedAdAccount { signed, ad }: OwnedAdAccount,
    State(ctx): State<AppContext>,
    Extension(settings): Extension<Arc<Settings>>,
    ViewEngine(v): ViewEngine<TeraView>,
    Path(id): Path<String>,
    Query(query): Query<WindowQuery>,
) -> Result<Response> {
    let text = signed.user.language.text()?;
    let store = WorkspaceStore::new(&ctx.db, &signed.workspace);
    let path = account_path(&id);
    if !store.monitoring_access(settings.now().timestamp()).await? {
        let updates = store.earlier_summaries(&id, 0, UPDATES_SHOWN).await?;
        let mut wanted = Wanted::default();
        for update in &updates {
            wanted.text(&update.summary);
        }
        let translations = Translations::load(&store, signed.user.language, wanted).await?;
        return ended(text, &v, &store, &ad, &updates, translations).await;
    }
    let check = store.latest_deep_check(&id).await?;
    let latest = store.latest_update(&id).await?;
    let window = window_of(query.window.as_deref());
    let clock = clock(&store, &settings, &id).await?;
    let campaigns = store.campaigns(&id).await?;
    let names = campaigns
        .iter()
        .map(|campaign| (campaign.id, campaign.name.clone()))
        .collect::<BTreeMap<_, _>>();
    let history = store.history_state(&id).await?;
    let numbers = numbers(text, &store, &id, &clock, history.as_ref(), HISTORY_DAYS).await?;
    let earlier_summaries = store.earlier_summaries(&id, 1, UPDATES_SHOWN - 1).await?;
    let recommendations = store.recommendations_with_findings(&id).await?;
    let raw_changes = match &numbers {
        Some(numbers) => {
            numbers
                .source(&id, window)
                .raw_changes(&store, &names, None)
                .await?
        }
        None => Vec::new(),
    };
    let mut wanted = Wanted::default();
    if let Some(assessment) = check.as_ref().and_then(|check| check.assessment.as_ref()) {
        wanted.assessment(assessment);
    }
    if let Some(update) = &latest {
        wanted.update(update);
    }
    for update in &earlier_summaries {
        wanted.text(&update.summary);
    }
    for found in &recommendations {
        wanted.found(found);
    }
    chart_pages::RawChange::wanted(&raw_changes, &mut wanted);
    let linked = Campaigns::new(
        campaigns
            .iter()
            .map(|campaign| (campaign.id, campaign.name.as_str())),
        None,
        |campaign| with_window(&campaign_path(&id, campaign), window),
    )
    .translated(Translations::load(&store, signed.user.language, wanted).await?);
    let assessment = check.as_ref().and_then(|check| check.assessment.as_ref().map(|assessment| (check.day, assessment)))
        .map(|(day, assessment)| {
            let shown = view::assessment_text(text, day, assessment, &Translations::english())?;
            Ok::<_, Error>(json!({
                "result": shown.result,
                "period": shown.period,
                "headline": linked.parts(&assessment.headline, None),
                "sources": if assessment.market.as_ref().is_some_and(|market| !market.trim().is_empty()) {
                    view::market_sources(check.as_ref().and_then(|check| check.research.as_ref()))
                } else { Vec::new() },
                "sections": shown.sections.into_iter().map(|(label, items)| json!({
                    "label": label,
                    "items": items.iter().map(|item| json!({
                        "observation": linked.parts(&item.observation, None),
                        "explanation": item.explanation.as_ref().map(|explanation| linked.parts(explanation, None)),
                    })).collect::<Vec<_>>()
                })).collect::<Vec<_>>()
            }))
        }).transpose()?;
    let mut data = json!({
        "assessment": assessment,
        "ad": {"id": id, "name": ad.name},
        "settings_href": format!("{path}/settings"),
        "export": {"action": format!("{path}/export.pdf"), "window": window.days()},
        "window_days": window.days(),
        "update": latest
            .as_ref()
            .map(|update| hero(text, update, clock.today, &names, &linked))
            .transpose()?,
        "earlier": earlier(text, &earlier_summaries, &linked)?,
        "recommendations": open_recommendations(recommendations, None, &linked),
        "panel_open": query.recommendations.as_deref() == Some(PANEL_OPEN),
        "panel_href": panel_href(&path, window),
        "deep_checked": check.is_some(),
        "reading_failed": latest.is_none()
            && history.as_ref().is_some_and(|state| state.error.is_some()),
        "return_to": {"window": if window == Window::ThirtyDays { "30" } else { "7" }},
    });
    if let Some(numbers) = numbers {
        let total =
            metrics::window_metrics(&numbers.days, numbers.end, window, None, numbers.judged);
        let source = numbers.source(&id, window);
        let metric = TrendMetric::chosen(query.metric.as_deref(), numbers.judged);
        let shown = view::metrics(text, &total, &numbers.currency)?;
        data["window"] = chart_pages::window_view(text, &path, window, numbers.end, metric)?;
        data["metric_links"] = json!(chart_pages::metric_links(
            text,
            &path,
            window,
            &shown,
            metric,
            numbers.judged
        ));
        data["metrics"] = json!(shown);
        data["explain"] = json!(explain::charts(text, numbers.judged));
        data["metric_sparklines"] = json!(source.metric_sparklines(None)?);
        data["notice"] = json!(numbers.notice);
        data["campaign_headers"] = json!(view::campaign_headers(text, numbers.judged));
        data["campaigns"] = json!(campaign_rows(
            &id,
            &campaigns,
            &numbers,
            window,
            &source.sparklines()?
        )?);
        data["trend"] = json!(source.trend(
            None,
            metric,
            &source.changes(&raw_changes, linked.translations())?
        )?);
        if numbers.judged {
            let target = store.profile(&id).await?.target_cost;
            data["cost_weeks"] = json!(source.cost_weeks(target.filter(|cost| *cost > 0.0))?);
            data["delay_note"] = json!(breakdown_pages::delay_note(text, &store, &id).await?);
        }
        data["campaign_shares"] = json!(source.campaign_shares(&campaigns, |campaign| {
            with_window(&campaign_path(&id, campaign), window)
        })?);
        data["device_shares"] = json!(source.device_shares(&store, None).await?);
        data["search_share"] = json!(
            source
                .search_share(&store, &campaigns, None, |campaign| {
                    with_window(&campaign_path(&id, campaign), window)
                })
                .await?
        );
        data["breakdowns_href"] = json!(with_window(&format!("{path}/breakdowns"), window));
    }
    format::render().view(&v, "monitoring/ad_account.html", data)
}

async fn first_update(
    OwnedAdAccount { signed, ad }: OwnedAdAccount,
    State(ctx): State<AppContext>,
    Extension(settings): Extension<Arc<Settings>>,
    ViewEngine(v): ViewEngine<TeraView>,
    Path(id): Path<String>,
) -> Result<Response> {
    let text = signed.user.language.text()?;
    let store = WorkspaceStore::new(&ctx.db, &signed.workspace);
    if !store.monitoring_access(settings.now().timestamp()).await? {
        return Ok(not_found());
    }
    let waiting_until = std::time::Instant::now()
        + std::time::Duration::from_secs(settings.monitoring.first_update_wait_seconds);
    let (update, reading_failed) = loop {
        let update = store.latest_update(&id).await?;
        let failed = store
            .history_state(&id)
            .await?
            .is_some_and(|state| state.error.is_some());
        if update.is_some() || failed || std::time::Instant::now() >= waiting_until {
            break (update, failed);
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    };
    let clock = clock(&store, &settings, &id).await?;
    let campaigns = store.campaigns(&id).await?;
    let names = campaigns
        .iter()
        .map(|campaign| (campaign.id, campaign.name.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut wanted = Wanted::default();
    if let Some(update) = &update {
        wanted.update(update);
    }
    let linked = Campaigns::new(
        campaigns
            .iter()
            .map(|campaign| (campaign.id, campaign.name.as_str())),
        None,
        |campaign| campaign_path(&id, campaign),
    )
    .translated(Translations::load(&store, signed.user.language, wanted).await?);
    format::render().view(
        &v,
        "monitoring/first_update.html",
        json!({
            "ad": {"id": id, "name": ad.name},
            "update": update
                .as_ref()
                .map(|update| hero(text, update, clock.today, &names, &linked))
                .transpose()?,
            "reading_failed": reading_failed && update.is_none(),
            "return_to": {"window": "7"},
        }),
    )
}

fn campaign_rows(
    id: &str,
    campaigns: &[Campaign],
    numbers: &Numbers,
    window: Window,
    sparklines: &BTreeMap<i64, String>,
) -> Result<Vec<Value>> {
    let grouped = metrics::by_campaign(&numbers.days);
    let mut rows = campaigns
        .iter()
        .map(|campaign| {
            (
                campaign,
                metrics::window_metrics(
                    grouped
                        .get(&campaign.id)
                        .map(Vec::as_slice)
                        .unwrap_or_default(),
                    numbers.end,
                    window,
                    None,
                    numbers.judged,
                ),
            )
        })
        .filter(|(campaign, metrics)| view::listed(campaign, metrics))
        .collect::<Vec<_>>();
    rows.sort_by(|(a, a_metrics), (b, b_metrics)| {
        b_metrics
            .current
            .spend
            .total_cmp(&a_metrics.current.spend)
            .then(a.name.cmp(&b.name))
    });
    rows.into_iter()
        .map(|(campaign, metrics)| {
            Ok(json!({
                "name": campaign.name,
                "href": with_window(&campaign_path(id, campaign.id), window),
                "cells": view::campaign_cells(numbers.text, &metrics, &numbers.currency)?,
                "sparkline": sparklines.get(&campaign.id),
            }))
        })
        .collect()
}

async fn ended(
    text: &Text,
    v: &TeraView,
    store: &WorkspaceStore<'_>,
    ad: &AdAccount,
    updates: &[EarlierUpdate],
    translations: Translations,
) -> Result<Response> {
    let subscribed = store.subscription().await?.is_some();
    let trial_end = text.written.format.day_month(
        chrono::DateTime::from_timestamp(
            store.trial_started_at().await? + access::TRIAL_SECONDS,
            0,
        )
        .ok_or_else(|| Error::Message("Invalid Trial end".into()))?
        .date_naive(),
    );
    format::render().view(
        v,
        "monitoring/ad_account_ended.html",
        json!({
            "ad": {"id": ad.customer_id, "name": ad.name},
            "subscribed": subscribed,
            "trial_end": trial_end,
            "price": text.whole_money((crate::billing::SUBSCRIPTION_USD_CENTS / 100) as f64, "USD"),
            "limit": access::AD_ACCOUNT_LIMIT,
            "updates": earlier(
                text,
                updates,
                &Campaigns::new([], None, |_| String::new()).translated(translations),
            )?,
            "settings_href": format!("{}/settings", account_path(&ad.customer_id)),
        }),
    )
}

async fn campaign(
    OwnedAdAccount { signed, ad }: OwnedAdAccount,
    State(ctx): State<AppContext>,
    Extension(settings): Extension<Arc<Settings>>,
    ViewEngine(v): ViewEngine<TeraView>,
    Path((id, campaign_id)): Path<(String, i64)>,
    Query(query): Query<WindowQuery>,
) -> Result<Response> {
    let text = signed.user.language.text()?;
    let store = WorkspaceStore::new(&ctx.db, &signed.workspace);
    let campaigns = store.campaigns(&id).await?;
    let Some(campaign) = campaigns
        .iter()
        .find(|campaign| campaign.id == campaign_id)
        .cloned()
    else {
        return Ok(not_found());
    };
    if !store.monitoring_access(settings.now().timestamp()).await? {
        return Ok(Redirect::to(&account_path(&id)).into_response());
    }
    let window = window_of(query.window.as_deref());
    let clock = clock(&store, &settings, &id).await?;
    let path = campaign_path(&id, campaign_id);
    let history = store.history_state(&id).await?;
    let Some(numbers) = numbers(text, &store, &id, &clock, history.as_ref(), HISTORY_DAYS).await?
    else {
        return Ok(Redirect::to(&account_path(&id)).into_response());
    };
    let rules = store
        .campaign_rules(&id, numbers.state, &numbers.monitoring)
        .await?;
    let effective = rules.effective(campaign_id);
    let judged = effective.judged;
    let update_lines = store
        .campaign_update_lines(&id, campaign_id, UPDATES_SHOWN)
        .await?
        .into_iter()
        .filter(|(_, line, _)| rules.shows(campaign_id, line))
        .collect::<Vec<_>>();
    let total = metrics::window_metrics(
        &numbers.days,
        numbers.end,
        window,
        Some(campaign_id),
        judged,
    );
    let mut source = numbers.source(&id, window);
    source.judged = judged;
    let metric = TrendMetric::chosen(query.metric.as_deref(), judged);
    let shown = view::metrics(text, &total, &numbers.currency)?;
    let names = BTreeMap::from([(campaign_id, campaign.name.clone())]);
    let stored_changes = source
        .stored_changes(&store, follow_up::OTHER_CHANGE_DAYS)
        .await?;
    let raw_changes = source.trend_changes(&stored_changes, &names, Some(campaign_id))?;
    let follow_ups = follow_up::follow_ups(
        &stored_changes,
        &numbers.days,
        &names,
        &rules,
        &numbers.currency,
        numbers.end + 1,
    );
    let found = store.recommendations_with_findings(&id).await?;
    let other_findings = store
        .monitoring_findings(&id)
        .await?
        .into_iter()
        .filter(|finding| {
            finding.campaign_id == Some(campaign_id)
                && rules.shows(campaign_id, &finding.rule.detail)
                && !found.iter().any(|found| {
                    found.recommendation.finding_key == finding.rule.key
                        && found.recommendation.state == RecommendationState::Open
                })
        })
        .collect::<Vec<_>>();
    let mut wanted = Wanted::default();
    for (_, line, cause) in &update_lines {
        wanted.text(line);
        if let Some(cause) = cause {
            wanted.text(&cause.evidence);
        }
    }
    for found in &found {
        wanted.found(found);
    }
    for finding in &other_findings {
        wanted.text(&finding.rule.detail);
    }
    chart_pages::RawChange::wanted(&raw_changes, &mut wanted);
    let linked = Campaigns::new(
        campaigns
            .iter()
            .map(|campaign| (campaign.id, campaign.name.as_str())),
        Some(campaign_id),
        |campaign| with_window(&campaign_path(&id, campaign), window),
    )
    .translated(Translations::load(&store, signed.user.language, wanted).await?);
    let mut lines = Vec::new();
    for (day, line, cause) in &update_lines {
        lines.push(json!({
            "date": view::long_date(text, *day)?,
            "text": linked.parts(line, Some(campaign_id)),
            "cause": cause
                .as_ref()
                .map(|cause| causes::view(text, cause, &linked, Some(campaign_id))),
        }));
    }
    let changes = source.changes(&raw_changes, linked.translations())?;
    let change_list = changes
        .iter()
        .zip(&raw_changes)
        .map(|(change, raw)| {
            let follow_up = follow_ups
                .iter()
                .find(|found| found.change_day == raw.day && found.change == raw.text)
                .map(|found| follow_up::page_text(text, found, &numbers.currency))
                .transpose()?;
            Ok(json!({
                "date": view::day_month(text, change.day)?,
                "text": change.text,
                "english": change.english,
                "follow_up": follow_up,
            }))
        })
        .collect::<Result<Vec<_>>>()?;
    let findings = other_findings
        .iter()
        .map(|finding| linked.parts(&finding.rule.detail, finding.campaign_id))
        .collect::<Vec<_>>();
    let deep_check =
        breakdown_pages::deep_check(text, &store, &id, campaign_id, &numbers.currency, judged)
            .await?
            .map(|(date, sections)| json!({"date": date, "sections": sections}));
    format::render().view(
        &v,
        "monitoring/campaign.html",
        json!({
            "ad": {"id": id, "name": ad.name, "href": with_window(&account_path(&id), window)},
            "campaign": {"id": campaign_id, "name": campaign.name},
            "export": {"action": format!("{path}/export.pdf"), "window": window.days()},
            "lines": lines,
            "window": chart_pages::window_view(text, &path, window, numbers.end, metric)?,
            "explain": explain::charts(text, judged),
            "metric_links": chart_pages::metric_links(text, &path, window, &shown, metric, judged),
            "metrics": shown,
            "metric_sparklines": source.metric_sparklines(Some(campaign_id))?,
            "notice": view::campaign_notice(text, numbers.notice.clone(), judged),
            "alerts_off": !effective.alerts,
            "settings_href": campaign_settings::path(&id, campaign_id),
            "trend": source.trend(Some(campaign_id), metric, &changes)?,
            "cost_weeks": if judged { Some(source.campaign_cost_weeks(campaign_id, effective.target)?) } else { None },
            "device_shares": source.device_shares(&store, Some(campaign_id)).await?,
            "search_share": source
                .search_share(
                    &store,
                    std::slice::from_ref(&campaign),
                    Some(campaign_id),
                    |_| String::new(),
                )
                .await?,
            "network_shares": source.network_shares(&store, campaign_id).await?,
            "changes": change_list,
            "recommendations": open_recommendations(
                found
                    .into_iter()
                    .filter(|found| rules.shows_recommendation(&found.recommendation))
                    .collect(),
                Some(campaign_id),
                &linked,
            ),
            "panel_open": query.recommendations.as_deref() == Some(PANEL_OPEN),
            "findings": findings,
            "deep_check": deep_check,
            "return_to": {"window": if window == Window::ThirtyDays { "30" } else { "7" }, "campaign": campaign_id},
        }),
    )
}

#[derive(Deserialize)]
struct ReturnTo {
    #[serde(default)]
    window: Option<String>,
}

async fn update_not_useful(
    OwnedAdAccount { signed, .. }: OwnedAdAccount,
    State(ctx): State<AppContext>,
    Path((id, day)): Path<(String, String)>,
    Form(form): Form<ReturnTo>,
) -> Result<Response> {
    let store = WorkspaceStore::new(&ctx.db, &signed.workspace);
    let Ok(day) = chrono::NaiveDate::parse_from_str(&day, "%Y-%m-%d") else {
        return Ok(not_found());
    };
    if !store.mark_update_not_useful(&id, day_number(day)).await? {
        return Ok(not_found());
    }
    Ok(Redirect::to(&with_window(
        &account_path(&id),
        window_of(form.window.as_deref()),
    ))
    .into_response())
}

#[derive(Deserialize)]
struct RecommendationForm {
    finding_key: String,
    state: String,
    #[serde(default)]
    window: Option<String>,
    #[serde(default)]
    campaign: Option<i64>,
}

async fn close_recommendation(
    OwnedAdAccount { signed, .. }: OwnedAdAccount,
    State(ctx): State<AppContext>,
    Extension(settings): Extension<Arc<Settings>>,
    Path(id): Path<String>,
    Form(form): Form<RecommendationForm>,
) -> Result<Response> {
    let store = WorkspaceStore::new(&ctx.db, &signed.workspace);
    let state = match form.state.as_str() {
        "done" => RecommendationState::Done,
        "not_useful" => RecommendationState::NotUseful,
        _ => return Ok(StatusCode::UNPROCESSABLE_ENTITY.into_response()),
    };
    let today = clock(&store, &settings, &id).await?.today;
    if !store
        .close_recommendation(&id, &form.finding_key, state, today)
        .await?
    {
        return Ok(not_found());
    }
    let path = match form.campaign {
        Some(campaign) => campaign_path(&id, campaign),
        None => account_path(&id),
    };
    Ok(Redirect::to(&panel_href(&path, window_of(form.window.as_deref()))).into_response())
}

#[derive(Default, Serialize)]
struct SettingsValues {
    goal: String,
    target_cost: String,
    margin: String,
    customer_value: String,
    close_rate: String,
    target_return: String,
    note: String,
    judge: Option<bool>,
    name: String,
    accent_colour: String,
    weekly_email: Option<bool>,
}

#[derive(Serialize)]
struct FieldError {
    key: &'static str,
    field: &'static str,
    message: &'static str,
}

#[derive(Deserialize)]
struct SavedQuery {
    saved: Option<String>,
}

async fn settings_page(
    v: &TeraView,
    store: &WorkspaceStore<'_>,
    settings: &Settings,
    signed: &SignedIn,
    ad: &AdAccount,
    submitted: Option<(SettingsValues, Vec<FieldError>)>,
    saved: bool,
) -> Result<Response> {
    let text = signed.user.language.text()?;
    let id = &ad.customer_id;
    let clock = clock(store, settings, id).await?;
    let (state, judged) = store.conversion_tracking(id, clock.today - 1).await?;
    let Some(branding) = store.branding(id).await? else {
        return Ok(not_found());
    };
    let refused = submitted.is_some();
    let (values, errors) = match submitted {
        Some(submitted) => submitted,
        None => {
            let profile = store.profile(id).await?;
            (
                SettingsValues {
                    goal: profile.goal,
                    target_cost: profile
                        .target_cost
                        .map(|cost| format!("{cost:.2}"))
                        .unwrap_or_default(),
                    margin: profile
                        .economics
                        .margin
                        .map(|value| format!("{:.2}", value * 100.0))
                        .unwrap_or_default(),
                    customer_value: profile
                        .economics
                        .customer_value
                        .map(|value| format!("{value:.2}"))
                        .unwrap_or_default(),
                    close_rate: profile
                        .economics
                        .close_rate
                        .map(|value| format!("{:.2}", value * 100.0))
                        .unwrap_or_default(),
                    target_return: profile
                        .economics
                        .target_return
                        .map(|value| format!("{value:.2}"))
                        .unwrap_or_default(),
                    note: profile.note,
                    judge: Some(judged),
                    name: branding.name,
                    accent_colour: branding.accent_colour,
                    weekly_email: Some(store.monitoring_settings(id).await?.weekly_email),
                },
                Vec::new(),
            )
        }
    };
    let mut response = format::render().view(
        v,
        "monitoring/settings.html",
        json!({
            "ad": {"id": id, "name": ad.name, "href": account_path(id)},
            "values": values,
            "field_errors": errors.iter().map(|error| (error.key, error.message)).collect::<BTreeMap<_, _>>(),
            "errors": errors,
            "saved": saved,
            "tracking": match state {
                ConversionTracking::Working => "working",
                ConversionTracking::TooFewResults => "too_few_results",
                ConversionTracking::NotSetUp => "not_set_up",
            },
            "has_logo": branding.has_logo,
            "currency_mark": clock.currency.as_deref().map(|currency| text.written.format.currency_mark(currency)),
            "email": signed.user.email,
            "add": add_options(store, settings).await?,
        }),
    )?;
    if refused {
        *response.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
    }
    Ok(response)
}

async fn settings(
    OwnedAdAccount { signed, ad }: OwnedAdAccount,
    State(ctx): State<AppContext>,
    Extension(settings): Extension<Arc<Settings>>,
    ViewEngine(v): ViewEngine<TeraView>,
    Query(query): Query<SavedQuery>,
) -> Result<Response> {
    let store = WorkspaceStore::new(&ctx.db, &signed.workspace);
    settings_page(
        &v,
        &store,
        &settings,
        &signed,
        &ad,
        None,
        query.saved.is_some(),
    )
    .await
}

async fn save_settings(
    OwnedAdAccount { signed, ad }: OwnedAdAccount,
    State(ctx): State<AppContext>,
    Extension(settings): Extension<Arc<Settings>>,
    ViewEngine(v): ViewEngine<TeraView>,
    Path(id): Path<String>,
    mut multipart: Multipart,
) -> Result<Response> {
    let text = signed.user.language.text()?;
    let store = WorkspaceStore::new(&ctx.db, &signed.workspace);
    let mut values = SettingsValues::default();
    let mut logo = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| Error::Message("Invalid Settings upload".into()))?
    {
        let name = field.name().map(str::to_owned);
        if name.as_deref() == Some("logo") {
            let bytes = field
                .bytes()
                .await
                .map_err(|_| Error::Message("Invalid logo".into()))?;
            if !bytes.is_empty() {
                logo = Some(bytes);
            }
            continue;
        }
        let text = field
            .text()
            .await
            .map_err(|_| Error::Message("Invalid Settings field".into()))?;
        match name.as_deref() {
            Some("goal") => values.goal = text,
            Some("target_cost") => values.target_cost = text,
            Some("margin") => values.margin = text,
            Some("customer_value") => values.customer_value = text,
            Some("close_rate") => values.close_rate = text,
            Some("target_return") => values.target_return = text,
            Some("note") => values.note = text,
            Some("name") => values.name = text,
            Some("accent_colour") => values.accent_colour = text,
            Some("judge_by_conversions") if text == "yes" || text == "no" => {
                values.judge = Some(text == "yes");
            }
            Some("weekly_email") if text == "on" || text == "off" => {
                values.weekly_email = Some(text == "on");
            }
            _ => return Ok(StatusCode::UNPROCESSABLE_ENTITY.into_response()),
        }
    }
    let mut errors = Vec::new();
    let cost = TargetCost::parse(&values.target_cost);
    if cost == TargetCost::NotANumber {
        errors.push(FieldError {
            key: "target",
            field: "target",
            message: text.written.page_target_not_number,
        });
    }
    let mut economics = crate::monitoring::economics::Economics::default();
    for (key, value, destination, scale, limit) in [
        (
            "margin",
            &values.margin,
            &mut economics.margin,
            100.0,
            100.0,
        ),
        (
            "customer_value",
            &values.customer_value,
            &mut economics.customer_value,
            1.0,
            1e9,
        ),
        (
            "close_rate",
            &values.close_rate,
            &mut economics.close_rate,
            100.0,
            100.0,
        ),
        (
            "target_return",
            &values.target_return,
            &mut economics.target_return,
            1.0,
            1e4,
        ),
    ] {
        let parsed = TargetCost::parse(value);
        let message = match parsed {
            TargetCost::NotANumber => Some(text.written.page_economics_not_number),
            TargetCost::Amount(amount)
                if amount <= 0.0 || amount > limit || (scale == 1.0 && amount == limit) =>
            {
                Some(if scale == 100.0 {
                    text.written.page_economics_share_invalid
                } else if key == "customer_value" {
                    text.written.page_customer_value_invalid
                } else {
                    text.written.page_target_return_invalid
                })
            }
            _ => None,
        };
        if let Some(message) = message {
            errors.push(FieldError {
                key,
                field: key,
                message,
            });
        }
        *destination = parsed.amount().map(|amount| amount / scale);
    }
    let profile = Profile {
        goal: values.goal.clone(),
        target_cost: cost.amount(),
        note: values.note.clone(),
        economics,
    };
    if errors.is_empty() && !profile.valid() {
        errors.push(FieldError {
            key: "note",
            field: "note",
            message: text.written.page_profile_not_saved,
        });
    }
    if !crate::workspace::branding::valid(&values.name, &values.accent_colour) {
        errors.push(FieldError {
            key: "name",
            field: "brand-name",
            message: text.written.page_branding_name,
        });
    }
    let logo = match logo {
        Some(bytes) => {
            let logo = compressed_logo(bytes).await?;
            if logo.is_none() {
                errors.push(FieldError {
                    key: "logo",
                    field: "brand-logo",
                    message: text.written.page_logo_rule,
                });
            }
            logo
        }
        None => None,
    };
    if !errors.is_empty() {
        return settings_page(
            &v,
            &store,
            &settings,
            &signed,
            &ad,
            Some((values, errors)),
            false,
        )
        .await;
    }
    let today = clock(&store, &settings, &id).await?.today;
    let (state, _) = store.conversion_tracking(&id, today - 1).await?;
    let mut monitoring = store.monitoring_settings(&id).await?;
    if let Some(judge) = values.judge {
        monitoring.judge_by_conversions =
            judgement_override(judge, state == ConversionTracking::Working);
    }
    if let Some(weekly_email) = values.weekly_email {
        monitoring.weekly_email = weekly_email;
    }
    if !store
        .save_ad_account_settings(
            &id,
            &AdAccountSettings {
                profile: &profile,
                branding_name: &values.name,
                accent_colour: &values.accent_colour,
                logo: logo.as_deref(),
                monitoring: &monitoring,
            },
        )
        .await?
    {
        return Ok(not_found());
    }
    Ok(Redirect::to(&format!("{}/settings?saved=1", account_path(&id))).into_response())
}

async fn confirm_removal(
    OwnedAdAccount { ad, .. }: OwnedAdAccount,
    ViewEngine(v): ViewEngine<TeraView>,
    Path(id): Path<String>,
) -> Result<Response> {
    format::render().view(
        &v,
        "monitoring/remove.html",
        json!({"ad": {"id": id, "name": ad.name, "settings_href": format!("{}/settings", account_path(&id))}}),
    )
}

async fn remove(
    Authenticated(signed): Authenticated,
    State(ctx): State<AppContext>,
    Extension(settings): Extension<Arc<Settings>>,
    Path(id): Path<String>,
) -> Result<Response> {
    if let Some(refused) =
        super::control::remove_owned(&ctx, &settings, &signed, "accounts", &id, true).await?
    {
        return Ok(if refused.status() == StatusCode::NOT_FOUND {
            not_found()
        } else {
            refused
        });
    }
    Ok(Redirect::to("/ad-accounts").into_response())
}

pub fn routes() -> Routes {
    Routes::new()
        .add("/ad-accounts", get(index))
        .add("/ad-accounts/{id}", get(show))
        .add("/ad-accounts/{id}/first-update", get(first_update))
        .add(
            "/ad-accounts/{id}/updates/{day}/not-useful",
            post(update_not_useful),
        )
        .add(
            "/ad-accounts/{id}/recommendations",
            post(close_recommendation),
        )
        .add("/ad-accounts/{id}/breakdowns", get(breakdown_pages::show))
        .add("/ad-accounts/{id}/campaigns/{campaign}", get(campaign))
        .add(
            "/ad-accounts/{id}/campaigns/{campaign}/settings",
            get(campaign_settings::show).post(campaign_settings::save),
        )
        .add(
            "/ad-accounts/{id}/settings",
            get(settings)
                .post(save_settings)
                .layer(DefaultBodyLimit::max(BRANDING_BODY_LIMIT)),
        )
        .add(
            "/ad-accounts/{id}/remove",
            get(confirm_removal).post(remove),
        )
}
