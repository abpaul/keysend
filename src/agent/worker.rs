use std::collections::BTreeMap;

use async_trait::async_trait;
use loco_rs::{app::AppContext, bgworker::BackgroundWorker, Error, Result};
use serde::{Deserialize, Serialize};

use super::{
    data::{self, Scope},
    investigation::{self, Earlier, Journal, Reply, Request},
    Failure, State,
};
use crate::{
    settings::Settings,
    workspace::{
        history::{day_number, local_time},
        investigations::{
            Investigation, InvestigationFinish, InvestigationStep, InvestigationUsage,
        },
        Workspace, WorkspaceStore,
    },
};

#[derive(Clone, Deserialize, Serialize)]
pub struct InvestigationJob {
    pub workspace_id: String,
    pub investigation_id: i64,
}

pub struct InvestigationWorker {
    ctx: AppContext,
}

#[async_trait]
impl BackgroundWorker<InvestigationJob> for InvestigationWorker {
    fn build(ctx: &AppContext) -> Self {
        Self { ctx: ctx.clone() }
    }

    async fn perform(&self, job: InvestigationJob) -> Result<()> {
        run(&self.ctx, &job).await
    }
}

struct StoredJournal<'a, 'b> {
    store: &'a WorkspaceStore<'b>,
    id: i64,
    failed: bool,
}

#[async_trait]
impl Journal for StoredJournal<'_, '_> {
    async fn step(&mut self, step: &InvestigationStep) {
        if let Err(error) = self.store.save_investigation_step(self.id, step).await {
            tracing::warn!(%error, "Investigation could not save its progress");
            self.failed = true;
        }
    }

    async fn answering(&mut self) {
        if let Err(error) = self
            .store
            .set_investigation_state(self.id, State::Answering)
            .await
        {
            tracing::warn!(%error, "Investigation could not save its state");
            self.failed = true;
        }
    }

    async fn cancelled(&mut self) -> bool {
        match self.store.investigation_cancelled(self.id).await {
            Ok(cancelled) => cancelled || self.failed,
            Err(error) => {
                tracing::warn!(%error, "Investigation could not read cancellation");
                self.failed = true;
                true
            }
        }
    }
}

async fn fail(store: &WorkspaceStore<'_>, id: i64, failure: Failure, now: i64) -> Result<()> {
    store
        .finish_investigation(
            id,
            &InvestigationFinish {
                state: State::Failed,
                failure: Some(failure),
                report: None,
                usage: InvestigationUsage::default(),
            },
            now,
        )
        .await?;
    Ok(())
}

pub async fn run(ctx: &AppContext, job: &InvestigationJob) -> Result<()> {
    let settings = Settings::load(ctx)?;
    let workspace = Workspace::from_identity(job.workspace_id.clone());
    let store = WorkspaceStore::new(&ctx.db, &workspace);
    let result = run_stored(&store, &settings, job.investigation_id).await;
    if let Err(error) = &result {
        tracing::warn!(%error, "Investigation failed");
        fail(
            &store,
            job.investigation_id,
            Failure::Interrupted,
            settings.now().timestamp(),
        )
        .await?;
    }
    result
}

async fn run_stored(store: &WorkspaceStore<'_>, settings: &Settings, id: i64) -> Result<()> {
    let Some(row) = store.investigation(id).await? else {
        return Ok(());
    };
    if row.state.finished() {
        return Ok(());
    }
    let now = settings.now().timestamp();
    if matches!(row.state, State::Gathering | State::Answering) {
        let oldest = now.saturating_sub(
            settings
                .agent
                .seconds
                .saturating_mul(3)
                .min(i64::MAX as u64) as i64,
        );
        store
            .fail_interrupted_investigation(id, oldest, now)
            .await?;
        return Ok(());
    }
    if !store.claim_investigation(id, now).await? {
        return Ok(());
    }
    if !store.monitoring_access(settings.now().timestamp()).await? {
        return fail(store, id, Failure::NoAccess, now).await;
    }
    let started = std::time::Instant::now();
    let deadline =
        std::time::Duration::from_secs(settings.agent.seconds.saturating_mul(2).min(240));
    match tokio::time::timeout(deadline, run_claimed(store, settings, row, started)).await {
        Ok(result) => result,
        Err(_) => fail(store, id, Failure::Budget, settings.now().timestamp()).await,
    }
}

async fn run_claimed(
    store: &WorkspaceStore<'_>,
    settings: &Settings,
    row: Investigation,
    started: std::time::Instant,
) -> Result<()> {
    let id = row.id;
    let now = settings.now().timestamp();
    let scope = Scope::load(store, &row.accounts).await?;
    if scope.accounts().is_empty() {
        return fail(store, id, Failure::NoAccess, now).await;
    }
    let mut snapshots = BTreeMap::new();
    for account in scope.accounts() {
        let (_, zone) = store
            .ad_account_metadata(&account.customer_id)
            .await?
            .ok_or_else(|| Error::Message("Ad Account metadata is missing".into()))?;
        let end = day_number(local_time(settings.now(), &zone)?.date_naive()) - 1;
        snapshots.insert(
            account.handle,
            data::load(store, &account.customer_id, end).await?,
        );
    }
    let earlier = earlier(store, &row).await?;
    let mut journal = StoredJournal {
        store,
        id,
        failed: false,
    };
    let finished = investigation::investigate_started(
        settings,
        Some(store),
        &settings.agent,
        Request {
            question: &row.question,
            period: row.period,
            today: crate::language::english()
                .written
                .format
                .date_with_year(settings.now().date_naive()),
            scope: &scope,
            data: &snapshots,
            earlier,
            done: row.steps,
        },
        &mut journal,
        started,
    )
    .await;
    if journal.failed {
        return Err(Error::Message("Investigation journal failed".into()));
    }
    // Clarification also completes a run, without passing through the answer callback.
    if finished.state == State::Completed {
        store.set_investigation_state(id, State::Answering).await?;
    }
    store
        .finish_investigation(
            id,
            &InvestigationFinish {
                state: finished.state,
                failure: finished.failure,
                report: finished.reply.map(serde_json::to_value).transpose()?,
                usage: finished.usage,
            },
            settings.now().timestamp(),
        )
        .await?;
    Ok(())
}

async fn earlier(store: &WorkspaceStore<'_>, row: &Investigation) -> Result<Option<Earlier>> {
    let Some(id) = row.follows else {
        return Ok(None);
    };
    let Some(previous) = store.investigation(id).await? else {
        return Ok(None);
    };
    match previous
        .report
        .map(serde_json::from_value::<Reply>)
        .transpose()?
    {
        Some(Reply::Answer(report)) => Ok(Some(Earlier {
            question: previous.question,
            report,
        })),
        _ => Ok(None),
    }
}
