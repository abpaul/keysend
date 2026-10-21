//! The queue jobs of Optimize. An evaluation runs after a daily run that collected the
//! Account history, and the hourly reconciliation catches up evaluations that a failed run
//! missed and settles uncertain changes. No job calls a model: Signals, Proposals, the policy
//! and the Outcomes are code, so an evaluation without new data costs nothing.

use std::collections::BTreeMap;

use async_trait::async_trait;
use loco_rs::{app::AppContext, bgworker::BackgroundWorker, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{
    execute::{self, Ran},
    outcome,
    policy::Standing,
    propose::{self, Context, Decision},
    signals, Approval, Mode, ProposalState,
};
use crate::{
    agent::data,
    google_ads,
    settings::Settings,
    workspace::{
        history::{day_number, local_time, AccountChange},
        optimization::Execution,
        Workspace, WorkspaceStore,
    },
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "task", rename_all = "snake_case")]
pub enum Task {
    Evaluate { ad_account: String },
    Execute { proposal_id: i64 },
    Reconcile,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OptimizationJob {
    pub workspace_id: String,
    #[serde(flatten)]
    pub task: Task,
}

pub struct OptimizationWorker {
    ctx: AppContext,
}

#[async_trait]
impl BackgroundWorker<OptimizationJob> for OptimizationWorker {
    fn build(ctx: &AppContext) -> Self {
        Self { ctx: ctx.clone() }
    }

    async fn perform(&self, job: OptimizationJob) -> Result<()> {
        let result = run(&self.ctx, &job).await;
        if let Err(error) = &result {
            tracing::warn!(%error, "Optimization job failed");
        }
        result
    }
}

/// Changes that ran or waited for longer than this are settled by a read of the live state.
const UNFINISHED_SECONDS: i64 = 600;
/// The spend of yesterday against the 7 days before it is checked only above this amount.
const MIN_SPEND: f64 = 10.0;

pub async fn run(ctx: &AppContext, job: &OptimizationJob) -> Result<()> {
    let settings = Settings::load(ctx)?;
    let workspace = Workspace::from_identity(job.workspace_id.clone());
    let store = WorkspaceStore::new(&ctx.db, &workspace);
    let now = settings.now().timestamp();
    match &job.task {
        Task::Evaluate { ad_account } => {
            for proposal in evaluate(&settings, &store, ad_account, now).await? {
                OptimizationWorker::perform_later(
                    ctx,
                    OptimizationJob {
                        workspace_id: job.workspace_id.clone(),
                        task: Task::Execute {
                            proposal_id: proposal,
                        },
                    },
                )
                .await?;
            }
        }
        Task::Execute { proposal_id } => {
            execute_one(&settings, &store, *proposal_id, now).await?;
        }
        Task::Reconcile => {
            for proposal in store
                .unfinished_executions(now - UNFINISHED_SECONDS)
                .await?
            {
                let Some(found) = store.proposal(proposal).await? else {
                    continue;
                };
                match google_ads::Client::connect(&settings, &store, &found.ad_account).await {
                    Ok(client) => {
                        execute::reconcile(&store, &client, proposal, now).await?;
                    }
                    Err(_) => tracing::warn!("An uncertain change could not be read back"),
                }
            }
            for proposal in execute::waiting(&store).await? {
                OptimizationWorker::perform_later(
                    ctx,
                    OptimizationJob {
                        workspace_id: job.workspace_id.clone(),
                        task: Task::Execute {
                            proposal_id: proposal,
                        },
                    },
                )
                .await?;
            }
            for ad in store.ad_accounts().await? {
                let current = store.optimization_policy(&ad.customer_id).await?;
                if current.policy.mode == Mode::Investigate {
                    continue;
                }
                let due = store
                    .optimization_evaluation(&ad.customer_id)
                    .await?
                    .is_none_or(|(_, at)| {
                        at <= now - i64::from(current.policy.evaluate_every_days) * 86_400
                    });
                if due {
                    OptimizationWorker::perform_later(
                        ctx,
                        OptimizationJob {
                            workspace_id: job.workspace_id.clone(),
                            task: Task::Evaluate {
                                ad_account: ad.customer_id,
                            },
                        },
                    )
                    .await?;
                }
            }
        }
    }
    Ok(())
}

/// Today in the Ad Account's time zone, as a day number.
async fn today(settings: &Settings, store: &WorkspaceStore<'_>, id: &str) -> Result<Option<i64>> {
    let Some((_, zone)) = store.ad_account_metadata(id).await? else {
        return Ok(None);
    };
    Ok(Some(day_number(
        local_time(settings.now(), &zone)?.date_naive(),
    )))
}

async fn budgets_micros(store: &WorkspaceStore<'_>, id: &str) -> Result<i64> {
    Ok(store
        .campaigns(id)
        .await?
        .iter()
        .filter(|campaign| campaign.status == "ENABLED")
        .filter_map(|campaign| campaign.budget_micros)
        .sum())
}

/// Runs one approved change through Google Ads.
pub async fn execute_one(
    settings: &Settings,
    store: &WorkspaceStore<'_>,
    proposal: i64,
    now: i64,
) -> Result<Ran> {
    let Some(found) = store.proposal(proposal).await? else {
        return Ok(Ran::Waiting);
    };
    let Some(day) = today(settings, store, &found.ad_account).await? else {
        return Ok(Ran::Waiting);
    };
    let budgets = budgets_micros(store, &found.ad_account).await?;
    if !execute::live(settings, &found.draft.action) || found.state != ProposalState::Approved {
        // Refused before any connection, with the reason in the history.
        let simulated = super::ads::Simulated::new(Vec::new());
        return execute::run(settings, store, &simulated, proposal, day, budgets, now).await;
    }
    match google_ads::Client::connect(settings, store, &found.ad_account).await {
        Ok(client) => execute::run(settings, store, &client, proposal, day, budgets, now).await,
        // Google could not be reached: the change stays approved, and the hourly
        // reconciliation queues it again until it expires.
        Err(failure)
            if !matches!(
                failure,
                google_ads::Failure::Reconnect | google_ads::Failure::Disabled
            ) =>
        {
            Ok(Ran::Waiting)
        }
        Err(_) => {
            execute::suspend(store, &found.ad_account, "access", now).await?;
            store
                .set_proposal_state(
                    proposal,
                    ProposalState::Failed,
                    json!({"refusal": "access"}),
                    now,
                )
                .await?;
            Ok(Ran::Refused(super::policy::Refusal::Access))
        }
    }
}

/// One evaluation of an Ad Account. Returns the Proposals that the Autopilot authorized, for
/// execution.
pub async fn evaluate(
    settings: &Settings,
    store: &WorkspaceStore<'_>,
    id: &str,
    now: i64,
) -> Result<Vec<i64>> {
    if !store
        .ad_accounts()
        .await?
        .iter()
        .any(|ad| ad.customer_id == id)
        || !store.monitoring_access(now).await?
    {
        return Ok(Vec::new());
    }
    let current = store.optimization_policy(id).await?;
    if current.policy.mode == Mode::Investigate {
        return Ok(Vec::new());
    }
    let Some(today) = today(settings, store, id).await? else {
        return Ok(Vec::new());
    };
    let account = data::load(store, id, today - 1).await?;
    let data_day = account.days.iter().map(|day| day.day).max().unwrap_or(0);
    let shadow_recorded = current.policy.mode != Mode::Shadow
        || store
            .shadows(Some(id), 1)
            .await?
            .first()
            .is_some_and(|shadow| {
                shadow.evaluation.policy_version == current.version
                    && shadow.evaluation.data_day >= data_day
            });
    if store
        .optimization_evaluation(id)
        .await?
        .is_some_and(|(evaluated_day, at)| {
            shadow_recorded
                && evaluated_day >= data_day
                && at > now - i64::from(current.policy.evaluate_every_days) * 86_400
        })
    {
        // No new data and not due: no work, and no model call.
        return Ok(Vec::new());
    }
    let mut standing = current.standing;
    if current.policy.mode == Mode::Autopilot && standing == Standing::Active {
        let reason = if !signals::tracking_reliable(&account) {
            Some("tracking")
        } else if data_day < today - i64::from(current.policy.max_data_age_days) {
            Some("sync")
        } else if spend_jumped(&account, data_day, current.policy.stop_spend_rise_percent) {
            Some("spend")
        } else {
            None
        };
        if let Some(reason) = reason {
            execute::suspend(store, id, reason, now).await?;
            standing = Standing::Suspended;
        }
    }
    let current = store.optimization_policy(id).await?;
    for open in store
        .proposals(Some(id), &[ProposalState::Open], 50)
        .await?
    {
        if open.draft.expires_at <= now {
            store
                .set_proposal_state(open.id, ProposalState::Expired, json!({}), now)
                .await?;
        } else if !propose::still_holds(&open.draft, &account, &current.policy) {
            store
                .set_proposal_state(
                    open.id,
                    ProposalState::Superseded,
                    json!({"reason": "signal_cleared"}),
                    now,
                )
                .await?;
        }
    }
    let budgets = store.campaign_budgets(id).await?;
    let mut recent =
        execute::recent(store, id, today, now, current.policy.observation_days).await?;
    recent.budgets_micros = budgets_micros(store, id).await?;
    for change in &account.changes {
        if let (Some(campaign), Some(date)) = (
            change.campaign_id,
            chrono::NaiveDate::parse_from_str(
                change.changed_at.get(..10).unwrap_or_default(),
                "%Y-%m-%d",
            )
            .ok(),
        ) {
            let days = today - day_number(date);
            let entry = recent.last_change_days.entry(campaign).or_insert(days);
            *entry = (*entry).min(days);
        }
    }
    let open = store.open_proposal_keys(id).await?;
    let rejected = store
        .rejected_proposal_keys(id, now - propose::REJECTED_DAYS * 86_400)
        .await?;
    let decisions = propose::decide(
        &account,
        &Context {
            policy: &current.policy,
            version: current.version,
            standing,
            budgets: &budgets,
            recent: &recent,
            open: &open,
            rejected: &rejected,
            today,
            now,
            currency: &account.currency,
        },
    );
    if current.policy.mode == Mode::Shadow {
        let evaluation = super::shadow::Evaluation::new(
            &account,
            &current.policy,
            current.version,
            today,
            now,
            decisions.clone(),
        );
        store.save_shadow(id, &evaluation).await?;
        for shadow in store.shadows(Some(id), 100).await? {
            let previous: Vec<super::shadow::Observation> = store
                .latest_shadow_observation(shadow.id)
                .await?
                .map(|event| serde_json::from_value(event.detail["observations"].clone()))
                .transpose()?
                .unwrap_or_default();
            let window = i64::from(shadow.evaluation.policy.observation_days);
            let complete = shadow.evaluation.decisions.iter().all(|decision| {
                let Decision::Propose(draft) = decision else {
                    return true;
                };
                previous.iter().any(|observation| {
                    observation.campaign_id == draft.action.campaign_id()
                        && observation.settled_days >= window
                        && observation.data_day >= shadow.evaluation.day + window
                })
            });
            if complete {
                continue;
            }
            let observations = shadow.evaluation.observe(&account)?;
            if !observations.is_empty() {
                store
                    .record_shadow_event(
                        shadow.id,
                        "observation",
                        &json!({"observations": observations}),
                        now,
                    )
                    .await?;
            }
        }
        let mut holds = BTreeMap::<&str, i64>::new();
        let mut proposals = 0;
        for decision in &decisions {
            match decision {
                Decision::Hold(_, hold) => *holds.entry(hold.name()).or_default() += 1,
                Decision::Propose(_) => proposals += 1,
            }
        }
        store
            .save_optimization_evaluation(
                id,
                data_day,
                decisions.len() as i64,
                proposals,
                &json!(holds),
                now,
            )
            .await?;
        return Ok(Vec::new());
    }
    // The Autopilot authorizes no more changes than the day has left, so that a Proposal it
    // cannot run waits for the User instead of failing on the daily limit.
    let mut left = current
        .policy
        .changes_per_day
        .saturating_sub(recent.changes_today);
    let mut holds = BTreeMap::<&str, i64>::new();
    let mut proposals = 0;
    let mut authorized = Vec::new();
    for decision in &decisions {
        match decision {
            Decision::Hold(_, hold) => *holds.entry(hold.name()).or_default() += 1,
            Decision::Propose(draft) => {
                let Some(proposal) = store.save_proposal(id, draft, now).await? else {
                    continue;
                };
                proposals += 1;
                if draft.approval == Approval::Automatic
                    && settings.optimize.max_mode == Mode::Autopilot
                    && left > 0
                    && standing == Standing::Active
                    && execute::live(settings, &draft.action)
                    && !store.optimization_stopped().await?
                    && store
                        .authorize_proposal(proposal, &draft.fingerprint(id), now)
                        .await?
                {
                    authorized.push(proposal);
                    left -= 1;
                }
            }
        }
    }
    record_outcomes(
        store,
        id,
        &account,
        today,
        now,
        current.policy.observation_days,
    )
    .await?;
    store
        .save_optimization_evaluation(
            id,
            data_day,
            decisions.len() as i64,
            proposals,
            &json!(holds),
            now,
        )
        .await?;
    Ok(authorized)
}

/// The Ad Account's spend on its newest day rose past the limit against the 7 days before.
fn spend_jumped(account: &data::AccountData, day: i64, percent: u32) -> bool {
    let last = crate::monitoring::metrics::sum(&account.days, day, day, None).spend;
    let before = crate::monitoring::metrics::sum(&account.days, day - 7, day - 1, None).spend / 7.0;
    before >= MIN_SPEND && last > before * (1.0 + f64::from(percent) / 100.0)
}

/// Records the Outcome of each applied change in its observation window when it changed
/// since the last record.
async fn record_outcomes(
    store: &WorkspaceStore<'_>,
    id: &str,
    account: &data::AccountData,
    today: i64,
    now: i64,
    observation_days: u32,
) -> Result<()> {
    let window = i64::from(observation_days);
    let zone = store.ad_account_metadata(id).await?.map(|(_, zone)| zone);
    for (execution, state, draft) in store
        .executions(id, now - (window * 3 + 7) * 86_400)
        .await?
    {
        if state != ProposalState::Applied {
            continue;
        }
        let others = outcome_changes(
            &account.changes,
            &execution,
            &draft.action,
            window,
            zone.as_deref(),
        );
        let observed = outcome::evaluate(&draft, execution.day, account, window, others);
        let last = store
            .optimization_events(Some(id), Some(execution.proposal_id), 50)
            .await?
            .into_iter()
            .find(|event| event.kind == "outcome")
            .and_then(|event| event.detail["state"].as_str().map(str::to_owned));
        if last.as_deref() != Some(observed.state.name()) {
            store
                .record_optimization_event(
                    id,
                    Some(execution.proposal_id),
                    "outcome",
                    &json!({"state": observed.state.name(), "observed": observed, "today": today}),
                    now,
                )
                .await?;
        }
    }
    Ok(())
}

fn outcome_changes(
    changes: &[AccountChange],
    execution: &Execution,
    action: &super::Action,
    window: i64,
    zone: Option<&str>,
) -> usize {
    let relevant: Vec<_> = changes
        .iter()
        .filter(|change| {
            change
                .campaign_id
                .is_none_or(|id| id == action.campaign_id())
        })
        .filter(|change| {
            chrono::NaiveDate::parse_from_str(
                change.changed_at.get(..10).unwrap_or_default(),
                "%Y-%m-%d",
            )
            .map(day_number)
            .map_or(true, |day| {
                (execution.day - window..=execution.day + window).contains(&day)
            })
        })
        .collect();
    let matching = relevant
        .iter()
        .filter(|change| own_change(change, execution, action, zone))
        .count();
    relevant.len() - usize::from(matching == 1)
}

fn own_change(
    change: &AccountChange,
    execution: &Execution,
    action: &super::Action,
    zone: Option<&str>,
) -> bool {
    use super::Action;
    use chrono::TimeZone;

    if !execution.verified
        || execution.error.is_some()
        || execution.request_id.as_deref().is_none_or(str::is_empty)
        || change.operation != "UPDATE"
    {
        return false;
    }
    let Some((before, after)) = execution.before.as_ref().zip(execution.after.as_ref()) else {
        return false;
    };
    let changed = zone
        .and_then(|zone| zone.parse::<chrono_tz::Tz>().ok())
        .and_then(|zone| {
            let local =
                chrono::NaiveDateTime::parse_from_str(&change.changed_at, "%Y-%m-%d %H:%M:%S%.f")
                    .ok()?;
            zone.from_local_datetime(&local)
                .single()
                .map(|at| at.with_timezone(&chrono::Utc))
        });
    // The event keeps its fraction of a second: one a fraction after the finish is outside.
    let instant = |at: i64| chrono::DateTime::<chrono::Utc>::from_timestamp(at, 0);
    if !changed.is_some_and(|changed| {
        execution.finished_at.is_some_and(|finished| {
            finished >= execution.started_at
                && finished - execution.started_at <= UNFINISHED_SECONDS
                && instant(execution.started_at)
                    .zip(instant(finished))
                    .is_some_and(|(start, finish)| (start..=finish).contains(&changed))
        })
    }) {
        return false;
    }
    let (resource, resource_type, fields, old, new) = match action {
        Action::AdjustCampaignBudget {
            budget_id,
            from_micros,
            to_micros,
            ..
        } => {
            if ![before, after].iter().all(|state| {
                state["budget_id"].as_i64() == Some(*budget_id)
                    && state["budget_shared"].as_bool() == Some(false)
                    && state["budget_campaigns"].as_i64() == Some(1)
            }) || before["budget_micros"].as_i64() != Some(*from_micros)
                || after["budget_micros"].as_i64() != Some(*to_micros)
            {
                return false;
            }
            (
                format!(
                    "customers/{}/campaignBudgets/{budget_id}",
                    execution.ad_account
                ),
                "CAMPAIGN_BUDGET",
                ["amount_micros", "amountMicros"],
                json!({"campaignBudget":{"amountMicros":from_micros.to_string()}}),
                json!({"campaignBudget":{"amountMicros":to_micros.to_string()}}),
            )
        }
        Action::PauseCampaign { campaign_id } | Action::EnableCampaign { campaign_id } => {
            let (old, new) = if matches!(action, Action::PauseCampaign { .. }) {
                ("ENABLED", "PAUSED")
            } else {
                ("PAUSED", "ENABLED")
            };
            if before["status"].as_str() != Some(old) || after["status"].as_str() != Some(new) {
                return false;
            }
            (
                format!("customers/{}/campaigns/{campaign_id}", execution.ad_account),
                "CAMPAIGN",
                ["status", "status"],
                json!({"campaign":{"status":old}}),
                json!({"campaign":{"status":new}}),
            )
        }
        _ => return false,
    };
    change.change_resource_name.as_deref() == Some(resource.as_str())
        && change.resource_type == resource_type
        && fields.contains(&change.changed_fields.as_str())
        && serde_json::from_str::<serde_json::Value>(&change.old_value)
            .ok()
            .as_ref()
            == Some(&old)
        && serde_json::from_str::<serde_json::Value>(&change.new_value)
            .ok()
            .as_ref()
            == Some(&new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimize::{policy::Live, Action};

    fn applied() -> (Execution, Action, AccountChange) {
        let at = chrono::DateTime::parse_from_rfc3339("2026-10-12T01:00:00Z")
            .unwrap()
            .timestamp();
        let live = Live {
            campaign_id: 701,
            status: "ENABLED".into(),
            budget_id: Some(9001),
            budget_micros: Some(80_000_000),
            budget_shared: false,
            budget_campaigns: 1,
            bidding_strategy: "MAXIMIZE_CONVERSIONS".into(),
        };
        let mut before = json!(live);
        before.as_object_mut().unwrap().remove("campaign_id");
        let mut after = before.clone();
        after["budget_micros"] = json!(64_000_000);
        let execution = Execution {
            proposal_id: 1,
            ad_account: "4829153307".into(),
            day: day_number(chrono::NaiveDate::from_ymd_opt(2026, 10, 11).unwrap()),
            automatic: false,
            before: Some(before),
            after: Some(after),
            request_id: Some("request".into()),
            error: None,
            verified: true,
            started_at: at,
            finished_at: Some(at),
        };
        let action = Action::AdjustCampaignBudget {
            campaign_id: 701,
            budget_id: 9001,
            from_micros: 80_000_000,
            to_micros: 64_000_000,
        };
        let change = AccountChange {
            changed_at: "2026-10-11 15:00:00".into(),
            resource_name: "customers/4829153307/changeEvents/1".into(),
            change_resource_name: Some("customers/4829153307/campaignBudgets/9001".into()),
            resource_type: "CAMPAIGN_BUDGET".into(),
            operation: "UPDATE".into(),
            campaign_id: Some(701),
            changed_fields: "amount_micros".into(),
            old_value: json!({"campaignBudget":{"amountMicros":"80000000"}}).to_string(),
            new_value: json!({"campaignBudget":{"amountMicros":"64000000"}}).to_string(),
        };
        (execution, action, change)
    }

    #[test]
    fn an_ad_accounts_own_budget_change_uses_its_timezone_and_target() {
        let (execution, action, change) = applied();
        assert_eq!(
            outcome_changes(&[change], &execution, &action, 14, Some("Pacific/Honolulu")),
            0
        );
    }

    #[test]
    fn an_ad_accounts_own_change_a_fraction_of_a_second_into_the_execution_is_its_own() {
        let (mut execution, action, mut change) = applied();
        execution.finished_at = Some(execution.started_at + 1);
        change.changed_at = "2026-10-11 15:00:00.123456".into();
        assert_eq!(
            outcome_changes(&[change], &execution, &action, 14, Some("Pacific/Honolulu")),
            0
        );
    }

    #[test]
    fn an_ad_accounts_own_status_change_requires_the_exact_status_transition() {
        for (action, old, new) in [
            (
                Action::PauseCampaign { campaign_id: 701 },
                "ENABLED",
                "PAUSED",
            ),
            (
                Action::EnableCampaign { campaign_id: 701 },
                "PAUSED",
                "ENABLED",
            ),
        ] {
            let (mut execution, _, mut change) = applied();
            execution.before.as_mut().unwrap()["status"] = json!(old);
            execution.after.as_mut().unwrap()["status"] = json!(new);
            change.change_resource_name = Some("customers/4829153307/campaigns/701".into());
            change.resource_type = "CAMPAIGN".into();
            change.changed_fields = "status".into();
            change.old_value = json!({"campaign":{"status":old}}).to_string();
            change.new_value = json!({"campaign":{"status":new}}).to_string();
            assert_eq!(
                outcome_changes(&[change], &execution, &action, 14, Some("Pacific/Honolulu")),
                0
            );
        }
    }

    #[test]
    fn an_ambiguous_ad_account_clock_keeps_the_outcome_for_manual_review() {
        let (mut execution, action, mut change) = applied();
        execution.started_at = chrono::DateTime::parse_from_rfc3339("2026-11-01T05:30:00Z")
            .unwrap()
            .timestamp();
        execution.finished_at = Some(execution.started_at);
        execution.day = day_number(chrono::NaiveDate::from_ymd_opt(2026, 11, 1).unwrap());
        change.changed_at = "2026-11-01 01:30:00".into();
        assert_eq!(
            outcome_changes(&[change], &execution, &action, 14, Some("America/New_York")),
            1
        );
    }

    #[test]
    fn an_ad_accounts_ambiguous_or_different_change_still_confounds_an_outcome() {
        let (execution, action, change) = applied();
        for altered in [
            AccountChange {
                change_resource_name: None,
                ..change.clone()
            },
            AccountChange {
                change_resource_name: Some("customers/4829153307/campaignBudgets/9002".into()),
                ..change.clone()
            },
            AccountChange {
                changed_fields: "amount_micros,name".into(),
                ..change.clone()
            },
            AccountChange {
                old_value: "{}".into(),
                ..change.clone()
            },
            AccountChange {
                new_value: "{}".into(),
                ..change.clone()
            },
            AccountChange {
                operation: "CREATE".into(),
                ..change.clone()
            },
            AccountChange {
                changed_at: "2026-10-11 15:01:00".into(),
                ..change.clone()
            },
            AccountChange {
                changed_at: "2026-10-11 15:00:00.123456".into(),
                ..change.clone()
            },
            AccountChange {
                changed_at: "2026-10-11 14:59:59.999999".into(),
                ..change.clone()
            },
        ] {
            assert_eq!(
                outcome_changes(
                    &[altered],
                    &execution,
                    &action,
                    14,
                    Some("Pacific/Honolulu")
                ),
                1
            );
        }
        assert_eq!(
            outcome_changes(std::slice::from_ref(&change), &execution, &action, 14, None),
            1
        );
        assert_eq!(
            outcome_changes(
                &[change.clone(), change],
                &execution,
                &action,
                14,
                Some("Pacific/Honolulu")
            ),
            2
        );
    }
}
