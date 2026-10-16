use async_trait::async_trait;
use chrono::{DateTime, Months, Timelike, Utc};
use loco_rs::{app::AppContext, bgworker::BackgroundWorker, Error, Result};
use serde::{Deserialize, Serialize};

use super::access;
use crate::{
    identity, removal,
    settings::Settings,
    workspace::{
        history::{day_number, local_time},
        Workspace, WorkspaceStore,
    },
};

pub const INACTIVE_WORKSPACE_MONTHS: u32 = 12;
pub const HISTORY_MONTHS: u32 = 24;
pub const RUN_HOUR_UTC: u32 = 3;

#[derive(Clone, Default, Deserialize, Serialize)]
pub struct RetentionJob {}

pub struct RetentionWorker {
    ctx: AppContext,
}

#[async_trait]
impl BackgroundWorker<RetentionJob> for RetentionWorker {
    fn build(ctx: &AppContext) -> Self {
        Self { ctx: ctx.clone() }
    }
    async fn perform(&self, _job: RetentionJob) -> Result<()> {
        let result = run(&self.ctx).await;
        if let Err(error) = &result {
            tracing::warn!(error = %error, "Retention job failed");
        }
        result
    }
}

pub fn due(now: DateTime<Utc>) -> bool {
    now.hour() == RUN_HOUR_UTC
}

fn months_before(now: DateTime<Utc>, months: u32) -> Result<DateTime<Utc>> {
    now.checked_sub_months(Months::new(months))
        .ok_or_else(|| Error::Message("Invalid retention date".into()))
}

pub async fn run(ctx: &AppContext) -> Result<()> {
    let settings = Settings::load(ctx)?;
    let now = settings.now();
    let inactive = remove_inactive_workspaces(ctx, &settings, now).await;
    let trials = identity::remove_trial_history_before(
        &ctx.db,
        months_before(now, HISTORY_MONTHS)?.timestamp(),
    )
    .await;
    let history = remove_old_history(ctx, now).await;
    inactive.and(trials).and(history)
}

async fn access_ended_before(store: &WorkspaceStore<'_>, oldest: i64) -> Result<bool> {
    let ended = access::access_ended_at(
        store.trial_started_at().await?,
        store.subscription().await?.as_ref(),
    );
    Ok(ended < oldest)
}

async fn remove_inactive_workspaces(
    ctx: &AppContext,
    settings: &Settings,
    now: DateTime<Utc>,
) -> Result<()> {
    let oldest = months_before(now, INACTIVE_WORKSPACE_MONTHS)?.timestamp();
    for id in identity::workspace_ids(&ctx.db).await? {
        match identity::signed_in(&ctx.db, &id).await {
            Ok(false) => {}
            Ok(true) => continue,
            Err(error) => {
                tracing::warn!(error = %error, "Retention could not read the sessions of a Workspace");
                continue;
            }
        }
        let workspace = Workspace::from_identity(id);
        let store = WorkspaceStore::new(&ctx.db, &workspace);
        match access_ended_before(&store, oldest).await {
            Ok(false) => continue,
            Ok(true) => {}
            Err(error) => {
                tracing::warn!(error = %error, "Retention could not read the Access of a Workspace");
                continue;
            }
        }
        match removal::delete_workspace(ctx, settings, &workspace).await {
            Ok(None) => tracing::info!("Retention deleted a Workspace whose Access ended"),
            Ok(Some(refusal)) => tracing::warn!(
                ?refusal,
                "Retention kept a Workspace whose Access ended; it tries again on the next run"
            ),
            Err(error) => tracing::warn!(
                error = %error,
                "Retention could not delete a Workspace whose Access ended; it tries again on the next run"
            ),
        }
    }
    Ok(())
}

async fn remove_old_history(ctx: &AppContext, now: DateTime<Utc>) -> Result<()> {
    let mut failure = None;
    for id in identity::workspace_ids(&ctx.db).await? {
        let workspace = Workspace::from_identity(id);
        let store = WorkspaceStore::new(&ctx.db, &workspace);
        if let Err(error) = remove_workspace_history(&store, now).await {
            tracing::warn!(error = %error, "Retention could not delete old history of a Workspace");
            failure.get_or_insert(error);
        }
    }
    failure.map_or(Ok(()), Err)
}

async fn remove_workspace_history(store: &WorkspaceStore<'_>, now: DateTime<Utc>) -> Result<()> {
    for ad in store.ad_accounts().await? {
        let Some((_, zone)) = store.ad_account_metadata(&ad.customer_id).await? else {
            continue;
        };
        let oldest = local_time(now, &zone)?
            .date_naive()
            .checked_sub_months(Months::new(HISTORY_MONTHS))
            .ok_or_else(|| Error::Message("Invalid retention date".into()))?;
        store
            .remove_history_before(
                &ad.customer_id,
                day_number(oldest),
                &format!("{oldest} 00:00:00"),
            )
            .await?;
    }
    Ok(())
}
