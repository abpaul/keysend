use std::{
    collections::BTreeMap,
    sync::{Arc, OnceLock},
};

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
    campaign_deep_check, campaign_export_charts, campaign_sparklines, export_changes,
    export_charts, first_read_day, metric_sparklines, Collected,
};
use crate::{
    app::not_found,
    auth::OwnedAdAccount,
    language::{say_plural, Language, Text},
    monitoring::{
        campaign_settings::Resolved,
        causes::{self, Cause},
        export::{
            self, CampaignRow, ChartImage, EarlierUpdate, Export, RecommendationText, Subject,
            UpdateLine, UpdateText,
        },
        format,
        metrics::{self, CampaignDay, Window},
        tracking,
        view::{self, Notice},
        UPDATES_SHOWN,
    },
    settings::Settings,
    translation::{Translations, Wanted},
    workspace::{
        history::{date_of, day_number, local_time, Campaign},
        monitoring::{RecommendationState, RecommendationWithFinding},
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

#[derive(Default, Deserialize)]
pub struct Params {
    pub window: Option<String>,
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
    notice: Option<Notice>,
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

    fn date(&self, day: i64) -> Result<String> {
        view::long_date(self.text, day)
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
    user_language: Language,
) -> Result<std::result::Result<Read, Response>> {
    let user = user_language.text()?;
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
    let language = match user_language {
        Language::Ro => Language::Ro,
        _ => Language::En,
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
    let text = language.text()?;
    Ok(Ok(Read {
        window,
        language,
        text,
        prepared: text.written.format.date_with_year(local.date_naive()),
        today,
        currency,
        end,
        first_day: day_number(first),
        days,
        judged,
        notice: view::tracking_notice(text, state, monitoring.judge_by_conversions, judged),
        rules,
        campaigns,
        campaign,
    }))
}

async fn translated(
    store: &WorkspaceStore<'_>,
    read: &Read,
    english: &[&str],
) -> Result<std::result::Result<Translations, Response>> {
    let mut wanted = Wanted::default();
    for text in english {
        wanted.text(text);
    }
    let translations = Translations::load(store, read.language, wanted).await?;
    let missing = english
        .iter()
        .filter(|text| translations.shown(text).english)
        .count();
    if missing > 0 {
        tracing::warn!(
            missing,
            language = read.language.code(),
            "Export refused: stored text has no translation yet"
        );
        return Ok(Err(refused(
            StatusCode::CONFLICT,
            read.text.written.export_refuse_untranslated,
        )));
    }
    Ok(Ok(translations))
}

fn open_recommendations(
    all: Vec<RecommendationWithFinding>,
    campaign: Option<i64>,
    rules: &Resolved,
) -> Vec<RecommendationWithFinding> {
    let mut open = all
        .into_iter()
        .filter(|found| {
            found.recommendation.state == RecommendationState::Open
                && campaign.is_none_or(|id| found.recommendation.campaign_id == Some(id))
                && rules.shows_recommendation(&found.recommendation)
        })
        .collect::<Vec<_>>();
    let spend = |found: &RecommendationWithFinding| {
        found
            .finding
            .as_ref()
            .map_or(f64::NEG_INFINITY, |finding| finding.spend)
    };
    open.sort_by(|a, b| spend(b).total_cmp(&spend(a)));
    open
}

fn recommendation_english(recommendations: &[RecommendationWithFinding]) -> Vec<&str> {
    recommendations
        .iter()
        .flat_map(|found| {
            [
                found.recommendation.action.as_str(),
                found.recommendation.reason.as_str(),
                found.recommendation.evidence.as_str(),
                found.recommendation.measure.as_str(),
            ]
            .into_iter()
            .chain(found.finding.as_ref().map(|finding| finding.title.as_str()))
        })
        .filter(|text| !text.trim().is_empty())
        .collect()
}

fn recommendation_texts(
    recommendations: &[RecommendationWithFinding],
    translations: &Translations,
) -> Vec<RecommendationText> {
    recommendations
        .iter()
        .map(|found| RecommendationText {
            subject: found.recommendation.subject.clone(),
            confidence: found.recommendation.confidence,
            finding: found
                .finding
                .as_ref()
                .map(|finding| translations.shown(&finding.title).text.into()),
            action: translations.shown(&found.recommendation.action).text.into(),
            reason: translations.shown(&found.recommendation.reason).text.into(),
            measure: translations
                .shown(&found.recommendation.measure)
                .text
                .into(),
            evidence: translations
                .shown(&found.recommendation.evidence)
                .text
                .into(),
        })
        .collect()
}

struct Content {
    cells: Vec<format::Cell>,
    sparklines: Vec<(&'static str, String)>,
    notice: Option<Notice>,
    charts: Vec<ChartImage>,
    recommendations: Vec<RecommendationText>,
    subject: Subject,
}

async fn assemble(
    store: &WorkspaceStore<'_>,
    id: &str,
    read: &Read,
    content: Content,
) -> Result<Export> {
    let branding = store
        .branding(id)
        .await?
        .ok_or_else(|| Error::Message("Branding is missing".into()))?;
    let length = read.window.days();
    let date = |day: i64| date_of(day).ok_or_else(|| Error::Message("Invalid Export day".into()));
    Ok(Export {
        sample: false,
        name: branding.name,
        accent: branding.accent_colour,
        logo: store.logo(id).await?,
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
        notice: content.notice,
        cells: content.cells,
        sparklines: content.sparklines,
        charts: content.charts,
        recommendations: content.recommendations,
        subject: content.subject,
    })
}

async fn render(
    store: &WorkspaceStore<'_>,
    export: Export,
    text: &'static Text,
) -> Result<Response> {
    let started = std::time::Instant::now();
    let result = render_pdf(export, text).await;
    store
        .record_operation(
            crate::workspace::measurement::Operation::Export,
            "",
            result.is_ok(),
            started.elapsed(),
            Default::default(),
        )
        .await;
    if result.is_ok() {
        store
            .record_activity(crate::workspace::measurement::Activity::Exported)
            .await;
    }
    result
}

async fn render_pdf(export: Export, text: &'static Text) -> Result<Response> {
    let file = file_name(
        text,
        &match &export.subject {
            Subject::AdAccount { .. } => export.name.clone(),
            Subject::Campaign { name, .. } => format!("{} {name}", export.name),
        },
        export.window_days,
    )?;
    let permit = RENDERS
        .get_or_init(|| Arc::new(Semaphore::new(2)))
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| Error::Message("Rendering is unavailable".into()))?;
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

fn campaign_rows(read: &Read, sparklines: &BTreeMap<i64, String>) -> Result<Vec<CampaignRow>> {
    let grouped = metrics::by_campaign(&read.days);
    let mut rows = read
        .campaigns
        .iter()
        .map(|campaign| {
            (
                campaign,
                metrics::window_metrics(
                    grouped
                        .get(&campaign.id)
                        .map(Vec::as_slice)
                        .unwrap_or_default(),
                    read.end,
                    read.window,
                    None,
                    read.rules.effective(campaign.id).judged,
                ),
            )
        })
        .filter(|(campaign, movement)| view::listed(campaign, movement))
        .collect::<Vec<_>>();
    rows.sort_by(|(a, a_movement), (b, b_movement)| {
        b_movement
            .current
            .spend
            .total_cmp(&a_movement.current.spend)
            .then_with(|| a.name.cmp(&b.name))
    });
    rows.into_iter()
        .map(|(campaign, movement)| {
            Ok(CampaignRow {
                name: campaign.name.clone(),
                sparkline: sparklines.get(&campaign.id).cloned(),
                cells: view::all_campaign_cells(read.text, &movement, &read.currency, read.judged)?,
            })
        })
        .collect()
}

pub type Built = std::result::Result<(Export, &'static Text), Response>;

/// The Export of an Ad Account with the text it is written in, or the response that refuses it.
pub async fn ad_account_export(
    store: &WorkspaceStore<'_>,
    settings: &Settings,
    id: &str,
    params: &Params,
    user_language: Language,
) -> Result<Built> {
    let read = match read(store, settings, id, None, params, user_language).await? {
        Ok(read) => read,
        Err(response) => return Ok(Err(response)),
    };
    let check = store.latest_deep_check(id).await?;
    let assessment = check.as_ref().and_then(|check| {
        check
            .assessment
            .as_ref()
            .map(|assessment| (check.day, assessment))
    });
    let rules = &read.rules;
    let total = metrics::window_metrics(&read.days, read.end, read.window, None, read.judged);
    let updates = store
        .updates(id, read.today - read.window.days() + 1, read.today)
        .await?;
    let latest = updates
        .first()
        .filter(|update| update.day > read.today - Window::SevenDays.days());
    let earlier = updates
        .iter()
        .skip(usize::from(latest.is_some()))
        .collect::<Vec<_>>();
    let recommendations =
        open_recommendations(store.recommendations_with_findings(id).await?, None, rules);
    let raw_changes =
        export_changes(store, &read.collected(id, &Translations::english()), None).await?;
    let earlier = view::earlier(
        read.text,
        &earlier,
        |update| update.day,
        |update| update.detection.quiet(),
    )?;
    let lines = latest
        .map(|update| {
            update
                .lines
                .iter()
                .filter(|line| rules.shows(line.campaign_id, &line.text))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut english = recommendation_english(&recommendations);
    if let Some((_, assessment)) = assessment {
        english.extend(crate::translation::assessment_texts(assessment));
    }
    if let Some(update) = latest {
        english.push(&update.summary);
        english.extend(update.cause.as_ref().map(|cause| cause.evidence.as_str()));
    }
    for line in &lines {
        english.push(&line.text);
        english.extend(line.cause.as_ref().map(|cause| cause.evidence.as_str()));
    }
    for item in &earlier {
        if let view::Earlier::One(update) = item {
            english.push(&update.summary);
        }
    }
    english.extend(raw_changes.iter().map(|change| change.text.as_str()));
    let translations = match translated(store, &read, &english).await? {
        Ok(translations) => translations,
        Err(response) => return Ok(Err(response)),
    };
    let name_of = |campaign: i64| {
        read.campaigns
            .iter()
            .find(|found| found.id == campaign)
            .map(|found| found.name.clone())
    };
    let update = latest
        .map(|update| {
            Ok::<_, Error>(UpdateText {
                date: read.date(update.day)?,
                summary: translations.shown(&update.summary).text.into(),
                cause: update
                    .cause
                    .as_ref()
                    .map(|cause| cause_text(read.text, cause, &translations)),
                lines: lines
                    .iter()
                    .map(|line| {
                        let campaign = update
                            .detection
                            .campaigns
                            .iter()
                            .find(|campaign| campaign.campaign_id == line.campaign_id)
                            .map(|campaign| campaign.name.clone())
                            .or_else(|| name_of(line.campaign_id))
                            .ok_or_else(|| {
                                Error::Message("Update names an unknown Campaign".into())
                            })?;
                        Ok(UpdateLine {
                            campaign,
                            text: translations.shown(&line.text).text.into(),
                            cause: line
                                .cause
                                .as_ref()
                                .map(|cause| cause_text(read.text, cause, &translations)),
                        })
                    })
                    .collect::<Result<_>>()?,
                more_campaigns: update.more_campaigns,
            })
        })
        .transpose()?;
    let earlier = earlier
        .into_iter()
        .map(|item| {
            Ok(match item {
                view::Earlier::One(update) => EarlierUpdate {
                    date: read.date(update.day)?,
                    summary: translations.shown(&update.summary).text.into(),
                },
                view::Earlier::Quiet(date) => EarlierUpdate {
                    date,
                    summary: read.text.earlier_quiet.into(),
                },
            })
        })
        .collect::<Result<_>>()?;
    let collected = read.collected(id, &translations);
    let (charts, deep_charts) = export_charts(store, &collected, &raw_changes, rules).await?;
    let content = Content {
        cells: format::cells(read.text, &total, &read.currency)?,
        sparklines: metric_sparklines(&collected, None)?,
        notice: read.notice.clone(),
        charts,
        recommendations: recommendation_texts(&recommendations, &translations),
        subject: Subject::AdAccount {
            assessment: assessment
                .map(|(day, assessment)| {
                    let mut shown =
                        view::assessment_text(read.text, day, assessment, &translations)?;
                    if assessment
                        .market
                        .as_ref()
                        .is_some_and(|market| !market.trim().is_empty())
                    {
                        shown.sources = view::market_sources(
                            check.as_ref().and_then(|check| check.research.as_ref()),
                        );
                    }
                    Ok::<_, Error>(Box::new(shown))
                })
                .transpose()?,
            update,
            earlier,
            campaign_headers: view::all_campaign_headers(read.text, read.judged),
            campaigns: campaign_rows(&read, &campaign_sparklines(&collected)?)?,
            deep_charts,
        },
    };
    Ok(Ok((assemble(store, id, &read, content).await?, read.text)))
}

/// The Export of one Campaign with the text it is written in, or the response that refuses it.
pub async fn campaign_export(
    store: &WorkspaceStore<'_>,
    settings: &Settings,
    id: &str,
    campaign: i64,
    params: &Params,
    user_language: Language,
) -> Result<Built> {
    let read = match read(store, settings, id, Some(campaign), params, user_language).await? {
        Ok(read) => read,
        Err(response) => return Ok(Err(response)),
    };
    let campaign = read
        .campaign
        .clone()
        .ok_or_else(|| Error::Message("Campaign is missing".into()))?;
    let judged = read.rules.effective(campaign.id).judged;
    let total =
        metrics::window_metrics(&read.days, read.end, read.window, Some(campaign.id), judged);
    let update_lines = store
        .campaign_update_lines(id, campaign.id, UPDATES_SHOWN)
        .await?
        .into_iter()
        .filter(|(_, text, _)| read.rules.shows(campaign.id, text))
        .collect::<Vec<_>>();
    let found = store.recommendations_with_findings(id).await?;
    let findings = store
        .monitoring_findings(id)
        .await?
        .into_iter()
        .filter(|finding| {
            finding.campaign_id == Some(campaign.id)
                && read.rules.shows(campaign.id, &finding.rule.detail)
                && !found.iter().any(|found| {
                    found.recommendation.finding_key == finding.rule.key
                        && found.recommendation.state == RecommendationState::Open
                })
        })
        .collect::<Vec<_>>();
    let recommendations = open_recommendations(found, Some(campaign.id), &read.rules);
    let raw_changes = export_changes(
        store,
        &Collected {
            judged,
            ..read.collected(id, &Translations::english())
        },
        Some(&campaign),
    )
    .await?;
    let mut english = recommendation_english(&recommendations);
    for (_, text, cause) in &update_lines {
        english.push(text);
        english.extend(cause.as_ref().map(|cause| cause.evidence.as_str()));
    }
    english.extend(findings.iter().map(|finding| finding.rule.detail.as_str()));
    english.extend(raw_changes.iter().map(|change| change.text.as_str()));
    let translations = match translated(store, &read, &english).await? {
        Ok(translations) => translations,
        Err(response) => return Ok(Err(response)),
    };
    let mut lines = Vec::new();
    for (day, text, cause) in &update_lines {
        lines.push((
            read.date(*day)?,
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
        ..read.collected(id, &translations)
    };
    let content = Content {
        cells: format::cells(read.text, &total, &read.currency)?,
        sparklines: metric_sparklines(&collected, Some(campaign.id))?,
        notice: view::campaign_notice(read.text, read.notice.clone(), judged),
        charts: campaign_export_charts(store, &collected, &campaign, &raw_changes, &read.rules)
            .await?,
        recommendations: recommendation_texts(&recommendations, &translations),
        subject: Subject::Campaign {
            deep_check: campaign_deep_check(store, &collected, &campaign, judged).await?,
            findings: findings
                .iter()
                .map(|finding| translations.shown(&finding.rule.detail).text.into())
                .collect(),
            name: campaign.name,
            lines,
        },
    };
    Ok(Ok((assemble(store, id, &read, content).await?, read.text)))
}

async fn export_pdf(
    OwnedAdAccount { signed, .. }: OwnedAdAccount,
    State(ctx): State<AppContext>,
    Extension(settings): Extension<Arc<Settings>>,
    Path(id): Path<String>,
    Query(params): Query<Params>,
) -> Result<Response> {
    let store = WorkspaceStore::new(&ctx.db, &signed.workspace);
    match ad_account_export(&store, &settings, &id, &params, signed.user.language).await? {
        Ok((export, text)) => render(&store, export, text).await,
        Err(response) => Ok(response),
    }
}

async fn campaign_export_pdf(
    OwnedAdAccount { signed, .. }: OwnedAdAccount,
    State(ctx): State<AppContext>,
    Extension(settings): Extension<Arc<Settings>>,
    Path((id, campaign)): Path<(String, i64)>,
    Query(params): Query<Params>,
) -> Result<Response> {
    let store = WorkspaceStore::new(&ctx.db, &signed.workspace);
    match campaign_export(
        &store,
        &settings,
        &id,
        campaign,
        &params,
        signed.user.language,
    )
    .await?
    {
        Ok((export, text)) => render(&store, export, text).await,
        Err(response) => Ok(response),
    }
}

pub fn routes() -> Routes {
    Routes::new()
        .add("/ad-accounts/{id}/export.pdf", get(export_pdf))
        .add(
            "/ad-accounts/{id}/campaigns/{campaign}/export.pdf",
            get(campaign_export_pdf),
        )
}
