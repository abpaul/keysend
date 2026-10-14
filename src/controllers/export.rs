use std::sync::{Arc, OnceLock};

use axum::{
    extract::{Path, Query},
    http::{header, StatusCode},
    response::IntoResponse,
    Extension,
};
use loco_rs::prelude::*;
use serde::Deserialize;
use tokio::sync::Semaphore;

use super::monitoring::{
    campaign_deep_check, campaign_export_charts, export_changes, export_charts, first_read_day,
    Collected, RawChange,
};
use crate::{
    app::not_found,
    auth::OwnedAdAccount,
    language::{say_plural, Language, Text},
    monitoring::{
        campaign_settings::Resolved,
        causes::{self, Cause},
        export::{self, CampaignRow, ChartImage, Export, RecommendationText, Subject, UpdateText},
        format,
        metrics::{self, CampaignDay, Window},
        tracking, view, UPDATES_SHOWN,
    },
    settings::Settings,
    translation::{Translations, Wanted},
    workspace::{
        history::{date_of, day_number, local_time, Campaign},
        monitoring::{Recommendation, RecommendationState},
        WorkspaceStore,
    },
};

static RENDERS: OnceLock<Arc<Semaphore>> = OnceLock::new();

fn cause_text(text: &Text, cause: &Cause, translations: &Translations) -> String {
    format!(
        "{}: {}",
        causes::label(text, cause.level),
        translations.shown(&cause.evidence).text
    )
}

fn with_cause(
    text: &Text,
    line: &str,
    cause: Option<&Cause>,
    translations: &Translations,
) -> String {
    match cause {
        Some(cause) => format!("{line} {}", cause_text(text, cause, translations)),
        None => line.to_owned(),
    }
}

#[derive(Deserialize)]
struct Params {
    window: Option<String>,
    language: Option<String>,
}

fn file_name(text: &Text, name: &str, days: i64) -> Result<String> {
    let slug = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let slug = if slug.is_empty() {
        text.written.export_file_fallback.to_owned()
    } else {
        slug
    };
    say_plural!(
        text,
        export_file,
        u64::try_from(days).unwrap_or_default(),
        name = slug
    )
}

struct Read {
    window: Window,
    language: Language,
    text: &'static Text,
    prepared: String,
    today: i64,
    currency: String,
    end: i64,
    first_day: i64,
    days: Vec<CampaignDay>,
    judged: bool,
    rules: Resolved,
    campaigns: Vec<Campaign>,
    campaign: Option<Campaign>,
}

impl Read {
    fn collected<'a>(&'a self, id: &'a str, translations: &'a Translations) -> Collected<'a> {
        Collected {
            translations,
            text: self.text,
            id,
            days: &self.days,
            campaigns: &self.campaigns,
            first_day: self.first_day,
            end: self.end,
            currency: &self.currency,
            judged: self.judged,
            window: self.window,
        }
    }
}

fn refused(status: StatusCode, message: &'static str) -> Response {
    (status, message).into_response()
}

async fn read(
    store: &WorkspaceStore<'_>,
    settings: &Settings,
    id: &str,
    campaign: Option<i64>,
    params: &Params,
    user: &'static Text,
    default_language: Language,
) -> Result<std::result::Result<Read, Response>> {
    let campaigns = store.campaigns(id).await?;
    let campaign = match campaign {
        Some(campaign) => {
            let Some(found) = campaigns.iter().find(|found| found.id == campaign).cloned() else {
                return Ok(Err(not_found()));
            };
            Some(found)
        }
        None => None,
    };
    let window = match params.window.as_deref() {
        None | Some("7") => Window::SevenDays,
        Some("30") => Window::ThirtyDays,
        Some(_) => {
            return Ok(Err(refused(
                StatusCode::UNPROCESSABLE_ENTITY,
                user.written.export_refuse_window,
            )))
        }
    };
    let language = match params.language.as_deref() {
        None => default_language,
        Some(code) => match Language::parse_available(code) {
            Some(language) => language,
            None => {
                return Ok(Err(refused(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    user.written.export_refuse_language,
                )))
            }
        },
    };
    let now = settings.now();
    if !store.monitoring_access(now.timestamp()).await? {
        return Ok(Err(refused(
            StatusCode::PAYMENT_REQUIRED,
            user.written.export_refuse_trial_ended,
        )));
    }
    let unread = || {
        Ok(Err(refused(
            StatusCode::CONFLICT,
            user.written.export_refuse_unread,
        )))
    };
    let Some((currency, zone)) = store.ad_account_metadata(id).await? else {
        return unread();
    };
    let local = local_time(now, &zone)?;
    let today = day_number(local.date_naive());
    let Some((first, last)) = store
        .history_state(id)
        .await?
        .and_then(|state| state.first_day.zip(state.last_day))
    else {
        return unread();
    };
    let end = metrics::last_shown_day(day_number(last), today);
    let days = store.monitoring_days(id, first_read_day(end), end).await?;
    let monitoring = store.monitoring_settings(id).await?;
    let (state, judged) = tracking::of_days(
        &days,
        end,
        store.conversion_actions(id).await?,
        monitoring.judge_by_conversions,
    );
    let rules = store.campaign_rules(id, state, &monitoring).await?;
    Ok(Ok(Read {
        window,
        language,
        text: language.text()?,
        prepared: language
            .text()?
            .written
            .format
            .date_with_year(local.date_naive()),
        today,
        currency,
        end,
        first_day: day_number(first),
        days,
        judged,
        rules,
        campaigns,
        campaign,
    }))
}

fn open_recommendations(
    all: Vec<Recommendation>,
    campaign: Option<i64>,
    rules: &Resolved,
) -> Vec<Recommendation> {
    all.into_iter()
        .filter(|recommendation| {
            recommendation.state == RecommendationState::Open
                && campaign.is_none_or(|id| recommendation.campaign_id == Some(id))
                && rules.shows_recommendation(recommendation)
        })
        .collect()
}

fn recommendation_texts(
    recommendations: &[Recommendation],
    translations: &Translations,
) -> Vec<RecommendationText> {
    recommendations
        .iter()
        .map(|recommendation| RecommendationText {
            action: translations.shown(&recommendation.action).text.into(),
            reason: translations.shown(&recommendation.reason).text.into(),
            evidence: translations.shown(&recommendation.evidence).text.into(),
        })
        .collect()
}

fn long_date(text: &Text, day: i64) -> String {
    date_of(day)
        .map(|date| format::long_date(text, date))
        .unwrap_or_default()
}

struct Content {
    cells: Vec<format::Cell>,
    charts: Vec<ChartImage>,
    recommendations: Vec<RecommendationText>,
    subject: Subject,
}

async fn render(
    store: &WorkspaceStore<'_>,
    id: &str,
    read: &Read,
    content: Content,
) -> Result<Response> {
    let branding = store
        .branding(id)
        .await?
        .ok_or_else(|| Error::Message("Branding is missing".into()))?;
    let logo = store.logo(id).await?;
    let length = read.window.days();
    let date = |day: i64| date_of(day).ok_or_else(|| Error::Message("Invalid Export day".into()));
    let export = Export {
        name: branding.name,
        accent: branding.accent_colour,
        logo,
        window_days: length,
        campaign_names: read
            .campaigns
            .iter()
            .map(|campaign| campaign.name.clone())
            .collect(),
        range: format::date_range(
            read.text,
            date(read.end - length + 1)?,
            date(read.end)?,
            true,
        ),
        compared: format::date_range(
            read.text,
            date(read.end - length * 2 + 1)?,
            date(read.end - length)?,
            true,
        ),
        prepared: read.prepared.clone(),
        cells: content.cells,
        judged: read.judged,
        charts: content.charts,
        recommendations: content.recommendations,
        subject: content.subject,
    };
    let file = file_name(
        read.text,
        &match &export.subject {
            Subject::AdAccount { .. } => export.name.clone(),
            Subject::Campaign { name, .. } => format!("{} {name}", export.name),
        },
        length,
    )?;
    let permit = RENDERS
        .get_or_init(|| Arc::new(Semaphore::new(2)))
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| Error::Message("Rendering is unavailable".into()))?;
    let text = read.text;
    let bytes = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        export::pdf(&export, text)
    })
    .await
    .map_err(|_| Error::Message("Export rendering failed".into()))??;
    Ok((
        [
            (header::CONTENT_TYPE, "application/pdf".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{file}\""),
            ),
        ],
        bytes,
    )
        .into_response())
}

async fn export_pdf(
    OwnedAdAccount { signed, .. }: OwnedAdAccount,
    State(ctx): State<AppContext>,
    Extension(settings): Extension<Arc<Settings>>,
    Path(id): Path<String>,
    Query(params): Query<Params>,
) -> Result<Response> {
    let store = WorkspaceStore::new(&ctx.db, &signed.workspace);
    let user = signed.user.language.text()?;
    let read = match read(
        &store,
        &settings,
        &id,
        None,
        &params,
        user,
        signed.user.language,
    )
    .await?
    {
        Ok(read) => read,
        Err(response) => return Ok(response),
    };
    let rules = &read.rules;
    let judged = read.judged;
    let total = metrics::window_metrics(&read.days, read.end, read.window, None, judged);
    let grouped = metrics::by_campaign(&read.days);
    let mut campaigns = Vec::new();
    for campaign in &read.campaigns {
        let movement = metrics::window_metrics(
            grouped
                .get(&campaign.id)
                .map(Vec::as_slice)
                .unwrap_or_default(),
            read.end,
            read.window,
            None,
            rules.effective(campaign.id).judged,
        );
        if movement.current.serving() || movement.previous.serving() {
            campaigns.push((campaign.name.clone(), movement.current));
        }
    }
    campaigns.sort_by(|a, b| b.1.spend.total_cmp(&a.1.spend).then_with(|| a.0.cmp(&b.0)));
    let campaigns = campaigns
        .into_iter()
        .map(|(name, current)| CampaignRow {
            name,
            spend: read.text.money(current.spend, &read.currency),
            clicks: read.text.count(current.clicks),
            impressions: read.text.count(current.impressions),
            conversions: judged.then(|| {
                current.conversions.map_or_else(
                    || read.text.written.export_not_judged.into(),
                    |value| read.text.count(value),
                )
            }),
        })
        .collect();
    let latest = store
        .updates(&id, read.today - 6, read.today)
        .await?
        .into_iter()
        .next();
    let recommendations =
        open_recommendations(store.monitoring_recommendations(&id).await?, None, rules);
    let raw_changes =
        export_changes(&store, &read.collected(&id, &Translations::english()), None).await?;
    let mut wanted = Wanted::default();
    if let Some(update) = &latest {
        wanted.update(update);
    }
    for recommendation in &recommendations {
        wanted.recommendation(recommendation);
    }
    RawChange::wanted(&raw_changes, &mut wanted);
    let translations = Translations::load(&store, read.language, wanted).await?;
    let update = latest.map(|update| UpdateText {
        date: long_date(read.text, update.day),
        summary: translations.shown(&update.summary).text.into(),
        cause: update
            .cause
            .as_ref()
            .map(|cause| cause_text(read.text, cause, &translations)),
        lines: update
            .lines
            .iter()
            .filter(|line| rules.shows(line.campaign_id, &line.text))
            .map(|line| {
                with_cause(
                    read.text,
                    translations.shown(&line.text).text,
                    line.cause.as_ref(),
                    &translations,
                )
            })
            .collect(),
    });
    let (charts, deep_charts) =
        export_charts(&store, &read.collected(&id, &translations), &raw_changes).await?;
    let content = Content {
        cells: format::cells(read.text, &total, &read.currency)?,
        charts,
        recommendations: recommendation_texts(&recommendations, &translations),
        subject: Subject::AdAccount {
            update,
            campaigns,
            deep_charts,
        },
    };
    render(&store, &id, &read, content).await
}

async fn campaign_export_pdf(
    OwnedAdAccount { signed, .. }: OwnedAdAccount,
    State(ctx): State<AppContext>,
    Extension(settings): Extension<Arc<Settings>>,
    Path((id, campaign)): Path<(String, i64)>,
    Query(params): Query<Params>,
) -> Result<Response> {
    let user = signed.user.language.text()?;
    let store = WorkspaceStore::new(&ctx.db, &signed.workspace);
    let read = match read(
        &store,
        &settings,
        &id,
        Some(campaign),
        &params,
        user,
        signed.user.language,
    )
    .await?
    {
        Ok(read) => read,
        Err(response) => return Ok(response),
    };
    let campaign = read
        .campaign
        .clone()
        .ok_or_else(|| Error::Message("Campaign is missing".into()))?;
    let judged = read.rules.effective(campaign.id).judged;
    let total =
        metrics::window_metrics(&read.days, read.end, read.window, Some(campaign.id), judged);
    let update_lines = store
        .campaign_update_lines(&id, campaign.id, UPDATES_SHOWN)
        .await?
        .into_iter()
        .filter(|(_, text, _)| read.rules.shows(campaign.id, text))
        .collect::<Vec<_>>();
    let recommendations = open_recommendations(
        store.monitoring_recommendations(&id).await?,
        Some(campaign.id),
        &read.rules,
    );
    let raw_changes = export_changes(
        &store,
        &Collected {
            judged,
            ..read.collected(&id, &Translations::english())
        },
        Some(&campaign),
    )
    .await?;
    let mut wanted = Wanted::default();
    for (_, text, cause) in &update_lines {
        wanted.text(text);
        if let Some(cause) = cause {
            wanted.text(&cause.evidence);
        }
    }
    for recommendation in &recommendations {
        wanted.recommendation(recommendation);
    }
    RawChange::wanted(&raw_changes, &mut wanted);
    let translations = Translations::load(&store, read.language, wanted).await?;
    let mut lines = Vec::new();
    for (day, text, cause) in &update_lines {
        lines.push((
            view::long_date(read.text, *day)?,
            with_cause(
                read.text,
                translations.shown(text).text,
                cause.as_ref(),
                &translations,
            ),
        ));
    }
    let collected = Collected {
        judged,
        ..read.collected(&id, &translations)
    };
    let content = Content {
        cells: format::cells(read.text, &total, &read.currency)?,
        charts: campaign_export_charts(&store, &collected, &campaign, &raw_changes).await?,
        recommendations: recommendation_texts(&recommendations, &translations),
        subject: Subject::Campaign {
            deep_check: campaign_deep_check(&store, &collected, &campaign, judged).await?,
            name: campaign.name,
            lines,
        },
    };
    render(&store, &id, &read, content).await
}

pub fn routes() -> Routes {
    Routes::new()
        .add("/ad-accounts/{id}/export.pdf", get(export_pdf))
        .add(
            "/ad-accounts/{id}/campaigns/{campaign}/export.pdf",
            get(campaign_export_pdf),
        )
}
