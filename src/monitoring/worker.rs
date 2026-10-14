use async_trait::async_trait;
use chrono::{Datelike, Days, Timelike};
use loco_rs::{app::AppContext, bgworker::BackgroundWorker, Error, Result};
use serde::{Deserialize, Serialize};

use super::{
    alerts,
    campaign_settings::{self, Resolved},
    causes, deep_check, detection, email, follow_up,
    metrics::{first_active_day, Window},
    update,
};
use crate::{
    google_ads,
    language::Language,
    settings::Settings,
    translation::{self, Translations},
    workspace::{
        history::{day_number, local_time},
        monitoring::RecommendationState,
        Workspace, WorkspaceStore,
    },
};

#[derive(Clone, Deserialize, Serialize)]
pub struct MonitoringJob {
    pub workspace_id: String,
    pub ad_account: String,
    #[serde(default)]
    pub immediate: bool,
}
pub struct MonitoringWorker {
    ctx: AppContext,
}

#[async_trait]
impl BackgroundWorker<MonitoringJob> for MonitoringWorker {
    fn build(ctx: &AppContext) -> Self {
        Self { ctx: ctx.clone() }
    }
    async fn perform(&self, job: MonitoringJob) -> Result<()> {
        let result = run(&self.ctx, &job).await;
        if let Err(error) = &result {
            tracing::warn!(error = %error, "Monitoring job failed");
        }
        result
    }
}

pub async fn run(ctx: &AppContext, job: &MonitoringJob) -> Result<()> {
    let settings = Settings::load(ctx)?;
    let workspace = Workspace::from_identity(job.workspace_id.clone());
    let store = WorkspaceStore::new(&ctx.db, &workspace);
    if !store
        .ad_accounts()
        .await?
        .iter()
        .any(|ad| ad.customer_id == job.ad_account)
    {
        return Ok(());
    }
    if store.ad_account_metadata(&job.ad_account).await?.is_none() {
        google_ads::today(&settings, &store, &job.ad_account).await?;
    }
    let (_, zone) = store
        .ad_account_metadata(&job.ad_account)
        .await?
        .ok_or_else(|| Error::Message("Ad Account metadata is missing".into()))?;
    let local = local_time(settings.now(), &zone)?;
    if local.hour() < 4 && !job.immediate {
        return Ok(());
    }
    let today = local.date_naive();
    let day = day_number(today);
    let now = settings.now().timestamp();
    if !store
        .claim_monitoring_run(&job.ad_account, day, now)
        .await?
    {
        EmailWorker::perform_later(
            ctx,
            EmailJob {
                workspace_id: job.workspace_id.clone(),
            },
        )
        .await?;
        return Ok(());
    }
    let result = account(&settings, &store, &job.ad_account, day).await;
    store
        .finish_monitoring_run(&job.ad_account, day, now, result.is_ok())
        .await?;
    EmailWorker::perform_later(
        ctx,
        EmailJob {
            workspace_id: job.workspace_id.clone(),
        },
    )
    .await?;
    translate(&settings, &store, &job.ad_account).await;
    result
}

async fn translate(settings: &Settings, store: &WorkspaceStore<'_>, id: &str) {
    match store.user_language().await {
        Ok(Language::En) => {}
        Ok(language) => {
            translation::translate_account(settings, store, id, language).await;
        }
        Err(_) => tracing::warn!("Translation could not read the language of the user"),
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub struct TranslationJob {
    pub workspace_id: String,
}
pub struct TranslationWorker {
    ctx: AppContext,
}

#[async_trait]
impl BackgroundWorker<TranslationJob> for TranslationWorker {
    fn build(ctx: &AppContext) -> Self {
        Self { ctx: ctx.clone() }
    }
    async fn perform(&self, job: TranslationJob) -> Result<()> {
        translate_workspace(&self.ctx, &job).await
    }
}

pub async fn translate_workspace(ctx: &AppContext, job: &TranslationJob) -> Result<()> {
    let settings = Settings::load(ctx)?;
    let workspace = Workspace::from_identity(job.workspace_id.clone());
    let store = WorkspaceStore::new(&ctx.db, &workspace);
    for ad in store.ad_accounts().await? {
        translate(&settings, &store, &ad.customer_id).await;
    }
    Ok(())
}

const EMAIL_TRANSLATION_WAIT: std::time::Duration = std::time::Duration::from_secs(60);

async fn email_translations(
    settings: &Settings,
    store: &WorkspaceStore<'_>,
    language: Language,
    email: &email::WeeklyEmail,
) -> Result<Translations> {
    if language == Language::En {
        return Ok(Translations::english());
    }
    for block in &email.blocks {
        let wait = tokio::time::timeout(
            EMAIL_TRANSLATION_WAIT,
            translation::translate_account(settings, store, &block.ad_account, language),
        )
        .await;
        if wait.is_err() {
            tracing::warn!("Weekly email translation timed out; the email keeps the English text");
        }
    }
    let translations = Translations::load(store, language, email::wanted(email)).await?;
    if email::untranslated(email, &translations) > 0 {
        tracing::warn!(
            language = language.code(),
            "Weekly email has text with no translation; it is sent in English"
        );
    }
    Ok(translations)
}

async fn account(
    settings: &Settings,
    store: &WorkspaceStore<'_>,
    id: &str,
    day: i64,
) -> Result<()> {
    if !store
        .monitoring_history_open(settings.now().timestamp())
        .await?
    {
        return Ok(());
    }
    tokio::time::timeout(
        std::time::Duration::from_secs(540),
        google_ads::collect_history(settings, store, id),
    )
    .await
    .map_err(|_| Error::Message("Account history collection timed out".into()))??;
    if !store.monitoring_access(settings.now().timestamp()).await? {
        return Ok(());
    }
    let end = day - 1;
    let days = store.monitoring_days(id, end - 62, end).await?;
    let campaigns = store.campaigns(id).await?;
    let first = first_active_day(&days).unwrap_or(end);
    let rules = store.resolved_settings(id, end).await?;
    let (currency, _) = store
        .ad_account_metadata(id)
        .await?
        .ok_or_else(|| Error::Message("Ad Account metadata is missing".into()))?;
    let from = crate::workspace::history::date_of(end - 6)
        .ok_or_else(|| Error::Message("Invalid Update day".into()))?;
    let to = crate::workspace::history::date_of(end)
        .ok_or_else(|| Error::Message("Invalid Update day".into()))?;
    let changes = store.account_changes(id, from, to).await?;
    let name = store
        .ad_accounts()
        .await?
        .into_iter()
        .find(|ad| ad.customer_id == id)
        .map(|ad| ad.name)
        .unwrap_or_default();
    for alert_day in end - 2..=end {
        if store.alert_evaluated(id, alert_day).await? {
            continue;
        }
        let from = crate::workspace::history::date_of(alert_day - 6)
            .ok_or_else(|| Error::Message("Invalid Alert day".into()))?;
        let to = crate::workspace::history::date_of(alert_day)
            .ok_or_else(|| Error::Message("Invalid Alert day".into()))?;
        let changes = store.account_changes(id, from, to).await?;
        let mut found = alerts::detect(
            &days, &campaigns, alert_day, first, &rules, &name, &currency,
        );
        found.retain(|alert| {
            alert
                .campaign_id
                .is_none_or(|campaign| rules.effective(campaign).alerts)
        });
        if alerts::budget_raised(&changes, alert_day) {
            found.retain(|alert| alert.event_key != alerts::EventKey::SpendDoubled);
        }
        store
            .evaluate_alerts(id, alert_day, &found, settings.now().timestamp())
            .await?;
    }
    if !store.has_update(id, day).await? {
        let mut detection =
            detection::detect(&days, &campaigns, &changes, end, first, &rules, &currency);
        let earlier = crate::workspace::history::date_of(follow_up::earliest_change_day(day))
            .ok_or_else(|| Error::Message("Invalid follow-up day".into()))?;
        follow_up::attach(
            &mut detection,
            &days,
            &campaigns,
            &store.account_changes(id, earlier, to).await?,
            &rules,
        );
        let inputs = causes::gather(store, id, &days, &changes, &campaigns, &detection).await?;
        detection.causes = causes::candidates(&detection, &inputs);
        let update = update::generate(settings, detection, &rules).await?;
        store.save_update(id, &update).await?;
    }
    let deep_result = refresh_check(settings, store, id, day, &rules).await;
    if let Err(error) = deep_result {
        store.deep_check_failed(id, day, &error.to_string()).await?;
        tracing::warn!(error = %error, "Deep check failed; retry on the next local day");
    }
    Ok(())
}

async fn refresh_check(
    settings: &Settings,
    store: &WorkspaceStore<'_>,
    id: &str,
    day: i64,
    rules: &Resolved,
) -> Result<()> {
    let mut check = match store.latest_deep_check(id).await? {
        Some(check) if day - check.day < 7 => check,
        _ => {
            let check = deep_check::collect(settings, store, id, day).await?;
            store.refresh_deep_check(id, &check, &[]).await?;
            check
        }
    };
    if !check.recommendations_generated {
        let blocked = store.recommendation_blocked_keys(id, day).await?;
        let mut new_findings = check.clone();
        new_findings
            .findings
            .retain(|finding| !blocked.contains(&finding.rule.key));
        let recommendations = deep_check::recommendations(settings, &new_findings, rules).await?;
        check.recommendations_generated = true;
        store
            .refresh_deep_check(id, &check, &recommendations)
            .await?;
    }
    Ok(())
}

#[derive(Clone, Deserialize, Serialize)]
pub struct EmailJob {
    pub workspace_id: String,
}
pub struct EmailWorker {
    ctx: AppContext,
}

#[async_trait]
impl BackgroundWorker<EmailJob> for EmailWorker {
    fn build(ctx: &AppContext) -> Self {
        Self { ctx: ctx.clone() }
    }
    async fn perform(&self, job: EmailJob) -> Result<()> {
        send(&self.ctx, &job).await
    }
}

const WEEKLY_CUT_OFF_HOUR: u32 = 18;

pub async fn weekly_content(settings: &Settings, store: &WorkspaceStore<'_>) -> Result<()> {
    if settings.now().weekday() != chrono::Weekday::Mon
        || settings.now().hour() < 6
        || settings.now().timestamp() - store.trial_started_at().await? < 5 * 86400
    {
        return Ok(());
    }
    let today = settings.now().date_naive();
    let week = today
        .checked_sub_days(Days::new(today.weekday().num_days_from_monday().into()))
        .ok_or_else(|| Error::Message("Invalid Weekly email day".into()))?;
    let day = day_number(week);
    if store.weekly_email(day).await?.is_some() {
        return Ok(());
    }
    let mut blocks = Vec::new();
    let mut left_out = Vec::new();
    for ad in store.ad_accounts().await? {
        if !store
            .monitoring_settings(&ad.customer_id)
            .await?
            .weekly_email
        {
            continue;
        }
        let Some((currency, zone)) = store.ad_account_metadata(&ad.customer_id).await? else {
            continue;
        };
        let local_today = day_number(local_time(settings.now(), &zone)?.date_naive());
        if !store.has_update(&ad.customer_id, local_today).await? {
            left_out.push(ad.name);
            continue;
        }
        let end = local_today - 1;
        let metrics = store
            .window_metrics(&ad.customer_id, end, Window::SevenDays, None)
            .await?;
        let updates = store
            .updates(&ad.customer_id, local_today - 6, local_today)
            .await?;
        let recommendation = store
            .monitoring_recommendations(&ad.customer_id)
            .await?
            .into_iter()
            .find(|recommendation| recommendation.state == RecommendationState::Open);
        let campaigns = email::campaign_parts(
            &store.monitoring_days(&ad.customer_id, end - 6, end).await?,
            &store.campaigns(&ad.customer_id).await?,
            &metrics,
        );
        blocks.push(email::block(
            ad.customer_id,
            ad.name,
            currency,
            metrics,
            updates,
            recommendation,
            campaigns,
        ));
    }
    if !left_out.is_empty() && settings.now().hour() < WEEKLY_CUT_OFF_HOUR {
        return Ok(());
    }
    if !blocks.is_empty() {
        store
            .save_weekly_email(
                &email::content(day, blocks, left_out)?,
                settings.now().timestamp(),
            )
            .await?;
    }
    Ok(())
}

fn claim_stays<T>(delivery: &std::result::Result<T, email::DeliveryError>) -> bool {
    matches!(delivery, Err(email::DeliveryError::Unknown { .. }))
}

pub async fn send(ctx: &AppContext, job: &EmailJob) -> Result<()> {
    let settings = Settings::load(ctx)?;
    let workspace = Workspace::from_identity(job.workspace_id.clone());
    let store = WorkspaceStore::new(&ctx.db, &workspace);
    let now = settings.now().timestamp();
    for email in store.abandon_unconfirmed_weekly_emails(now).await? {
        tracing::error!(
            kind = "weekly",
            day = email.day,
            ad_accounts = ?email.blocks.iter().map(|block| block.ad_account.as_str()).collect::<Vec<_>>(),
            "Email sending was not confirmed; it is not sent again"
        );
    }
    for ad in store.ad_accounts().await? {
        for alert in store
            .abandon_unconfirmed_alerts(&ad.customer_id, now)
            .await?
        {
            tracing::error!(
                kind = "alert",
                ad_account = %ad.customer_id,
                day = alert.day,
                event = %alert.event_key,
                "Email sending was not confirmed; it is not sent again"
            );
        }
        store.expire_alerts(&ad.customer_id, now).await?;
    }
    store.expire_weekly_emails(now).await?;
    if !store.monitoring_access(settings.now().timestamp()).await? {
        return Ok(());
    }
    weekly_content(&settings, &store).await?;
    let Some(recipient) = store.user_email().await? else {
        return Ok(());
    };
    let language = store.user_language().await?;
    if settings.email_recipient.as_deref() != Some(&recipient)
        || settings.services.email_url.is_none()
        || settings.email_key.is_none()
    {
        return Ok(());
    }
    for ad in store.ad_accounts().await? {
        let alerts_on = store.monitoring_settings(&ad.customer_id).await?.alerts;
        let own = store.campaign_settings(&ad.customer_id).await?;
        let Some((currency, _)) = store.ad_account_metadata(&ad.customer_id).await? else {
            continue;
        };
        for alert in store.pending_alerts(&ad.customer_id).await? {
            if !campaign_settings::alerts_on(alerts_on, &own, alert.campaign_id) {
                continue;
            }
            if !store.claim_alert(&ad.customer_id, &alert, now).await? {
                continue;
            }
            let message = alerts::message(
                language,
                &alert,
                &ad.customer_id,
                &ad.name,
                &currency,
                &settings.origin,
            );
            let result = match message {
                Ok(message) => {
                    email::deliver(
                        &settings,
                        &recipient,
                        &message.subject,
                        &message.text,
                        Some(&message.html),
                    )
                    .await
                }
                Err(error) => Err(email::DeliveryError::Refused(error)),
            };
            if claim_stays(&result) {
                continue;
            }
            store
                .finish_alert_email(&ad.customer_id, &alert, now, result.is_ok())
                .await?;
            result?;
        }
    }
    if settings.now().weekday() != chrono::Weekday::Mon || settings.now().hour() < 6 {
        return Ok(());
    }
    for email in store.pending_weekly_emails().await? {
        if !store.claim_weekly_email(email.day, now).await? {
            continue;
        }
        let Some(email) = store.prepare_weekly_delivery(&email, now).await? else {
            continue;
        };
        let translations = match email_translations(&settings, &store, language, &email).await {
            Ok(translations) => translations,
            Err(_) => {
                tracing::warn!(
                    "Weekly email could not read its translations; it is sent in English"
                );
                Translations::empty(language)
            }
        };
        let result = match email::message(language, &email, &settings.origin, &translations) {
            Ok(message) => {
                email::deliver(
                    &settings,
                    &recipient,
                    &message.subject,
                    &message.text,
                    Some(&message.html),
                )
                .await
            }
            Err(error) => Err(email::DeliveryError::Refused(error)),
        };
        if claim_stays(&result) {
            continue;
        }
        store
            .finish_weekly_email(email.day, now, result.is_ok())
            .await?;
        result?;
    }
    Ok(())
}
