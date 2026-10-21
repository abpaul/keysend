use chrono::{DateTime, Timelike, Utc};
use loco_rs::{
    app::AppContext,
    bgworker::{BackgroundWorker, JobStatus},
    Result,
};

use super::{
    retention::{self, RetentionJob, RetentionWorker},
    worker::{MonitoringJob, MonitoringWorker},
};
use crate::{
    identity,
    workspace::{Workspace, WorkspaceStore},
};

pub async fn collect_history(ctx: &AppContext, workspace: &Workspace) -> Result<()> {
    for ad in WorkspaceStore::new(&ctx.db, workspace)
        .ad_accounts()
        .await?
    {
        MonitoringWorker::perform_later(
            ctx,
            MonitoringJob {
                workspace_id: workspace.id().into(),
                ad_account: ad.customer_id,
                immediate: false,
            },
        )
        .await?;
    }
    crate::optimize::cycle::OptimizationWorker::perform_later(
        ctx,
        crate::optimize::cycle::OptimizationJob {
            workspace_id: workspace.id().into(),
            task: crate::optimize::cycle::Task::Reconcile,
        },
    )
    .await?;
    Ok(())
}

pub async fn start_first_runs(
    ctx: &AppContext,
    workspace: &Workspace,
    ad_accounts: &[String],
) -> Result<()> {
    for ad_account in ad_accounts {
        MonitoringWorker::perform_later(
            ctx,
            MonitoringJob {
                workspace_id: workspace.id().into(),
                ad_account: ad_account.clone(),
                immediate: true,
            },
        )
        .await?;
    }
    Ok(())
}

const COMPLETED_JOB_DAYS: i64 = 7;
const FAILED_JOB_DAYS: i64 = 30;

/// Removes the old rows of the job queue. The queue keeps each job after it ends, and nothing
/// else removes it. A failed job stays longer, for diagnosis.
pub async fn remove_old_jobs(ctx: &AppContext) -> Result<()> {
    let Some(queue) = &ctx.queue_provider else {
        return Ok(());
    };
    queue
        .clear_jobs_older_than(COMPLETED_JOB_DAYS, &vec![JobStatus::Completed])
        .await?;
    queue
        .clear_jobs_older_than(FAILED_JOB_DAYS, &vec![JobStatus::Failed])
        .await
}

pub async fn queue_retention(ctx: &AppContext, now: DateTime<Utc>) -> Result<()> {
    if retention::due(now) {
        RetentionWorker::perform_later(ctx, RetentionJob {}).await?;
    }
    Ok(())
}

pub async fn snapshot_business(ctx: &AppContext, now: DateTime<Utc>) {
    crate::workspace::measurement::recorded(
        crate::workspace::measurement::snapshot(&ctx.db, now).await,
    );
}

pub async fn hourly(ctx: AppContext) {
    loop {
        snapshot_business(&ctx, crate::settings::wall_clock()).await;
        if remove_old_jobs(&ctx).await.is_err() {
            tracing::warn!("Old jobs could not be removed from the queue");
        }
        if queue_retention(&ctx, crate::settings::wall_clock())
            .await
            .is_err()
        {
            tracing::warn!("Daily run could not queue the retention job");
        }
        match identity::workspace_ids(&ctx.db).await {
            Ok(ids) => {
                for id in ids {
                    if collect_history(&ctx, &Workspace::from_identity(id))
                        .await
                        .is_err()
                    {
                        tracing::warn!("Daily run could not queue an Ad Account");
                    }
                }
            }
            Err(_) => tracing::warn!("Daily run could not read Workspaces"),
        }
        let now = crate::settings::wall_clock();
        tokio::time::sleep(std::time::Duration::from_secs(
            3600 - u64::from(now.minute() * 60 + now.second()),
        ))
        .await;
    }
}
