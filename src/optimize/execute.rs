//! Running one approved change.
//!
//! In order: the Proposal is approved and its Approval binds its fingerprint; nothing has
//! stopped the Workspace or the Ad Account; the kind is live in this installation; the
//! current policy still allows the change against the live state that Google Ads returns; the
//! store claims it within the limits of the day; Google Ads validates it; Google Ads applies
//! it; KeySend reads the state back. Only a read-back that shows the intended value makes the
//! change Applied. A change whose answer was lost is Uncertain until a read of the live state
//! confirms the intended value. An old value may be a stale read, so KeySend never resends it.

use serde_json::json;

use super::{
    ads::{self, Ads, AdsError},
    policy::{self, Live, Recent, Refusal, Standing, Verdict},
    Action, Approval, Mode, Proposal, ProposalState,
};
use crate::{
    settings::Settings,
    workspace::{
        optimization::{Claim, Finish, Suspend},
        WorkspaceStore,
    },
};
use loco_rs::Result;

/// What execution did with a Proposal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ran {
    /// Applied and read back.
    Applied,
    /// Refused before anything was sent, for this reason.
    Refused(Refusal),
    /// Not run now: another change of the Ad Account runs, or the Proposal is not approved.
    Waiting,
    /// Google Ads refused it or could not be reached; nothing changed.
    Failed(AdsError),
    /// Sent with no answer and not yet settled by a read.
    Uncertain,
    /// The live value changed in a way that neither the change nor the value before explains.
    Conflict,
}

/// The kinds that this installation may send to Google Ads. Applying is off unless
/// `settings.optimize.live` is set; then only the listed kinds run.
pub fn live(settings: &Settings, action: &Action) -> bool {
    settings.optimize.live
        && settings.optimize.max_mode >= Mode::Recommend
        && settings.optimize.actions.contains(&action.kind())
        && SENDABLE.contains(&action.kind())
}

/// The kinds that the Google Ads client can send. Settings cannot add another.
pub const SENDABLE: [super::ActionKind; 2] = [
    super::ActionKind::AdjustCampaignBudget,
    super::ActionKind::PauseCampaign,
];

fn raise(action: &Action) -> i64 {
    match action {
        Action::AdjustCampaignBudget {
            from_micros,
            to_micros,
            ..
        } => (to_micros - from_micros).max(0),
        _ => 0,
    }
}

fn state(live: &Live) -> serde_json::Value {
    json!({"status": live.status, "budget_micros": live.budget_micros, "budget_id": live.budget_id, "budget_shared": live.budget_shared, "budget_campaigns": live.budget_campaigns, "bidding_strategy": live.bidding_strategy})
}

async fn refuse(
    store: &WorkspaceStore<'_>,
    proposal: &Proposal,
    refusal: Refusal,
    now: i64,
) -> Result<Ran> {
    let next = if refusal == Refusal::Expired {
        ProposalState::Expired
    } else {
        ProposalState::Failed
    };
    let moved = store
        .set_proposal_state(proposal.id, next, json!({"refusal": refusal.name()}), now)
        .await?;
    Ok(if moved {
        Ran::Refused(refusal)
    } else {
        Ran::Waiting
    })
}

/// Suspends the Autopilot of an Ad Account, keeping its policy. Only the User resumes it.
pub async fn suspend(store: &WorkspaceStore<'_>, id: &str, reason: &str, now: i64) -> Result<()> {
    let current = store.optimization_policy(id).await?;
    if current.version > 0
        && current.standing == Standing::Active
        && current.policy.mode == Mode::Autopilot
    {
        store
            .save_optimization_policy(id, &current.policy, Standing::Suspended, reason, now)
            .await?;
    }
    Ok(())
}

/// What KeySend already changed today in the Ad Account, and when each Campaign last changed.
pub async fn recent(
    store: &WorkspaceStore<'_>,
    id: &str,
    day: i64,
    now: i64,
    observation_days: u32,
) -> Result<Recent> {
    recent_except(store, id, day, now, observation_days, None).await
}

async fn recent_except(
    store: &WorkspaceStore<'_>,
    id: &str,
    day: i64,
    now: i64,
    observation_days: u32,
    exclude: Option<i64>,
) -> Result<Recent> {
    let since = now - i64::from(observation_days.max(90)) * 86_400;
    let mut recent = Recent::default();
    for (execution, state, draft) in store.executions(id, since).await? {
        if Some(execution.proposal_id) == exclude {
            continue;
        }
        if !matches!(
            state,
            ProposalState::Applied | ProposalState::Uncertain | ProposalState::Conflict
        ) && state != ProposalState::Applying
        {
            continue;
        }
        let campaign = draft.action.campaign_id();
        let days = (now - execution.started_at) / 86_400;
        let entry = recent.last_change_days.entry(campaign).or_insert(days);
        *entry = (*entry).min(days);
        if execution.day == day {
            recent.changes_today += 1;
            recent.raised_today_micros += raise(&draft.action);
        }
        if days < i64::from(observation_days) {
            recent
                .observing
                .push((campaign, draft.action.may_spend_more()));
        }
    }
    Ok(recent)
}

/// Runs an approved Proposal. `day` is today in the Ad Account's time zone; `budgets` is the
/// sum of the daily budgets of its enabled Campaigns, for the spending envelope.
pub async fn run(
    settings: &Settings,
    store: &WorkspaceStore<'_>,
    ads: &dyn Ads,
    proposal: i64,
    day: i64,
    budgets_micros: i64,
    now: i64,
) -> Result<Ran> {
    let Some(proposal) = store.proposal(proposal).await? else {
        return Ok(Ran::Waiting);
    };
    if proposal.state != ProposalState::Approved {
        return Ok(Ran::Waiting);
    }
    let started = std::time::Instant::now();
    let action = &proposal.draft.action;
    if proposal.draft.approval == Approval::Automatic
        && settings.optimize.max_mode < Mode::Autopilot
    {
        return refuse(store, &proposal, Refusal::NotLive, now).await;
    }
    if !live(settings, action) {
        return refuse(store, &proposal, Refusal::NotLive, now).await;
    }
    if let Some(refusal) = still_allowed(store, &proposal, None, day, budgets_micros, now).await? {
        return refuse(store, &proposal, refusal, now).await;
    }
    let current = store.optimization_policy(&proposal.ad_account).await?;
    let automatic = proposal.draft.approval == Approval::Automatic;
    match store
        .claim_execution(
            &proposal,
            day,
            automatic,
            raise(action),
            current.policy.changes_per_day,
            current.policy.max_daily_increase_micros,
            now,
        )
        .await?
    {
        Claim::Claimed => {}
        Claim::NotApproved | Claim::Busy => return Ok(Ran::Waiting),
        Claim::DailyChanges => return refuse(store, &proposal, Refusal::DailyChanges, now).await,
        Claim::DailyRaise => return refuse(store, &proposal, Refusal::DailyRaise, now).await,
    }
    let mut before_state = serde_json::Value::Null;
    for validate in [true, false] {
        let read = ads.campaign(action.campaign_id()).await;
        if !store.execution_is_applying(proposal.id, now).await? {
            return Ok(Ran::Waiting);
        }
        let before = match read {
            Ok(Some(before)) => before,
            Ok(None) => {
                return finish(
                    store,
                    &proposal,
                    Some(now),
                    &Finish {
                        error: Some("not_found"),
                        ..ending(ProposalState::Conflict, None, None, None)
                    },
                    now,
                    Ran::Conflict,
                )
                .await;
            }
            Err(error) => {
                let text = serde_json::to_string(&error)?;
                return failure(
                    store,
                    &proposal,
                    &current.policy,
                    None,
                    error,
                    &text,
                    now,
                    now,
                )
                .await;
            }
        };
        before_state = state(&before);
        let checked_now = now.saturating_add(started.elapsed().as_secs() as i64);
        let refusal = still_allowed(
            store,
            &proposal,
            Some(&before),
            day,
            budgets_micros,
            checked_now,
        )
        .await?;
        // Reconciliation takes an interrupted execution out of Applying. Only the worker
        // that reserved this Proposal may send it, while that reservation is still active.
        if !store.execution_is_applying(proposal.id, now).await? {
            return Ok(Ran::Waiting);
        }
        if let Some(refusal) = refusal {
            let conflict = matches!(refusal, Refusal::Stale | Refusal::SharedBudget);
            let next = if conflict {
                ProposalState::Conflict
            } else {
                ProposalState::Failed
            };
            return finish(
                store,
                &proposal,
                Some(now),
                &Finish {
                    refusal: Some(refusal),
                    suspend: (automatic && conflict).then_some(Suspend::Because(refusal.name())),
                    ..ending(next, Some(&before_state), None, None)
                },
                checked_now,
                Ran::Refused(refusal),
            )
            .await;
        }
        if !validate {
            break;
        }
        if let Err(error) = ads.apply(action, true).await {
            if !store.execution_is_applying(proposal.id, now).await? {
                return Ok(Ran::Waiting);
            }
            let text = serde_json::to_string(&error)?;
            return failure(
                store,
                &proposal,
                &current.policy,
                Some(&before_state),
                error,
                &text,
                checked_now,
                now,
            )
            .await;
        }
    }
    if !store
        .mark_execution_dispatch(
            proposal.id,
            now,
            now.saturating_add(started.elapsed().as_secs() as i64),
        )
        .await?
    {
        return Ok(Ran::Waiting);
    }
    let request_id = match ads.apply(action, false).await {
        Ok(applied) => applied.request_id,
        Err(AdsError::Unknown) => None,
        Err(error) => {
            let text = serde_json::to_string(&error)?;
            return failure(
                store,
                &proposal,
                &current.policy,
                Some(&before_state),
                error,
                &text,
                now.saturating_add(started.elapsed().as_secs() as i64),
                now,
            )
            .await;
        }
    };
    // The read-back settles the change whether this worker or a reconciliation holds it.
    reconcile_with(
        store,
        ads,
        &proposal,
        Some(&before_state),
        request_id.as_deref(),
        now.saturating_add(started.elapsed().as_secs() as i64),
    )
    .await
}

/// A finish with no refusal and no suspension, verified only when Applied.
fn ending<'a>(
    state: ProposalState,
    before: Option<&'a serde_json::Value>,
    after: Option<&'a serde_json::Value>,
    request_id: Option<&'a str>,
) -> Finish<'a> {
    Finish {
        state,
        before,
        after,
        request_id,
        error: None,
        refusal: None,
        verified: state == ProposalState::Applied,
        suspend: None,
    }
}

#[allow(clippy::too_many_arguments)]
async fn failure(
    store: &WorkspaceStore<'_>,
    proposal: &Proposal,
    policy: &policy::Policy,
    before: Option<&serde_json::Value>,
    error: AdsError,
    text: &str,
    now: i64,
    claimed_at: i64,
) -> Result<Ran> {
    let suspend = if error == AdsError::Access {
        Suspend::Because("access")
    } else {
        Suspend::AfterFailures {
            since: now - 7 * 86_400,
            limit: policy.stop_after_failures,
        }
    };
    finish(
        store,
        proposal,
        Some(claimed_at),
        &Finish {
            error: Some(text),
            suspend: Some(suspend),
            ..ending(ProposalState::Failed, before, None, None)
        },
        now,
        Ran::Failed(error),
    )
    .await
}

/// Records the end of a change. The worker that claimed it at `claimed_at` finishes it only
/// while it still owns it; a reconciliation finishes it while it is running or uncertain. A
/// finish that does not move the Proposal changes nothing, its suspension included.
async fn finish(
    store: &WorkspaceStore<'_>,
    proposal: &Proposal,
    claimed_at: Option<i64>,
    finish: &Finish<'_>,
    now: i64,
    ran: Ran,
) -> Result<Ran> {
    let moved = match claimed_at {
        Some(claimed_at) => {
            store
                .finish_claimed_execution(proposal.id, claimed_at, finish, now)
                .await?
        }
        None => store.finish_execution(proposal.id, finish, now).await?,
    };
    Ok(if moved { ran } else { Ran::Waiting })
}

/// Settles a change by reading the live state: the intended value makes it Applied, the
/// value before remains Uncertain because reads may lag, anything else is a Conflict for the User.
/// Only the read that moves a running or uncertain Proposal suspends the Autopilot.
async fn reconcile_with(
    store: &WorkspaceStore<'_>,
    ads: &dyn Ads,
    proposal: &Proposal,
    before: Option<&serde_json::Value>,
    request_id: Option<&str>,
    now: i64,
) -> Result<Ran> {
    let action = &proposal.draft.action;
    let started = std::time::Instant::now();
    let read = ads.campaign(action.campaign_id()).await;
    let now = now.saturating_add(started.elapsed().as_secs() as i64);
    let after_state = match &read {
        Ok(Some(after)) => Some(state(after)),
        _ => None,
    };
    let (end, ran) = match &read {
        Ok(Some(after)) if ads::sets(action, after) == Some(true) => (
            ending(
                ProposalState::Applied,
                before,
                after_state.as_ref(),
                request_id,
            ),
            Ran::Applied,
        ),
        Ok(Some(after)) if ads::before(action, after) == Some(true) => (
            Finish {
                error: Some("unconfirmed"),
                ..ending(
                    ProposalState::Uncertain,
                    before,
                    after_state.as_ref(),
                    request_id,
                )
            },
            Ran::Uncertain,
        ),
        Ok(Some(_)) => (
            Finish {
                error: Some("changed"),
                suspend: Some(Suspend::Because("conflict")),
                ..ending(
                    ProposalState::Conflict,
                    before,
                    after_state.as_ref(),
                    request_id,
                )
            },
            Ran::Conflict,
        ),
        Ok(None) => (
            Finish {
                error: Some("not_found"),
                ..ending(ProposalState::Conflict, before, None, request_id)
            },
            Ran::Conflict,
        ),
        Err(error) => (
            Finish {
                error: Some("no_read_back"),
                suspend: (*error == AdsError::Access).then_some(Suspend::Because("access")),
                ..ending(ProposalState::Uncertain, before, None, request_id)
            },
            Ran::Uncertain,
        ),
    };
    finish(store, proposal, None, &end, now, ran).await
}

/// Settles an uncertain change, or one that stayed Applying, by reading the live state. It
/// never sends the change again.
pub async fn reconcile(
    store: &WorkspaceStore<'_>,
    ads: &dyn Ads,
    proposal: i64,
    now: i64,
) -> Result<Ran> {
    let Some(proposal) = store.proposal(proposal).await? else {
        return Ok(Ran::Waiting);
    };
    if !matches!(
        proposal.state,
        ProposalState::Uncertain | ProposalState::Applying
    ) {
        return Ok(Ran::Waiting);
    }
    if proposal.state == ProposalState::Applying {
        store
            .finish_execution(
                proposal.id,
                &Finish {
                    error: Some("interrupted"),
                    ..ending(ProposalState::Uncertain, None, None, None)
                },
                now,
            )
            .await?;
    }
    reconcile_with(store, ads, &proposal, None, None, now).await
}

/// Approved Proposals that no execution has claimed, in Ad Accounts where no change is
/// running: a change that waited for another one runs once that one is settled. The hourly
/// reconciliation queues them again.
pub async fn waiting(store: &WorkspaceStore<'_>) -> Result<Vec<i64>> {
    store.waiting_proposals().await
}

/// Rechecks the full policy and access against current evidence, excluding this Proposal's
/// reservation from the limits and cooldown it must satisfy.
async fn still_allowed(
    store: &WorkspaceStore<'_>,
    proposal: &Proposal,
    live: Option<&Live>,
    day: i64,
    budgets_micros: i64,
    now: i64,
) -> Result<Option<Refusal>> {
    if proposal.approved.as_deref() != Some(proposal.fingerprint.as_str())
        || proposal.draft.fingerprint(&proposal.ad_account) != proposal.fingerprint
    {
        return Ok(Some(Refusal::Fingerprint));
    }
    if proposal.draft.expires_at <= now {
        return Ok(Some(Refusal::Expired));
    }
    if store.optimization_stopped().await? {
        return Ok(Some(Refusal::Stopped));
    }
    if !store.monitoring_access(now).await? {
        return Ok(Some(Refusal::Access));
    }
    let current = store.optimization_policy(&proposal.ad_account).await?;
    if current.policy.mode == Mode::Shadow {
        return Ok(Some(Refusal::Shadow));
    }
    if current.policy.mode == Mode::Investigate {
        return Ok(Some(Refusal::Investigate));
    }
    let automatic = proposal.draft.approval == Approval::Automatic;
    if automatic && current.version != proposal.draft.policy_version {
        return Ok(Some(Refusal::PolicyChanged));
    }
    let action = &proposal.draft.action;
    if raise(action) > 0 {
        return Ok(Some(Refusal::BudgetTotalUnknown));
    }
    // A cut approved for a cost above the target is not sent once the profile's economics show
    // that the Campaign's conversions pay for themselves. Without a break-even cost nothing can
    // pay, so only a cut in an Ad Account with economics loads the account data. A load that
    // fails returns its error, so the change is not sent.
    if matches!(action, Action::AdjustCampaignBudget { from_micros, to_micros, .. } if to_micros < from_micros)
        && store
            .profile(&proposal.ad_account)
            .await?
            .economics
            .break_even_cost()
            .is_some()
    {
        let data = crate::agent::data::load(store, &proposal.ad_account, day - 1).await?;
        if super::propose::cut_pays(&proposal.draft, &data) {
            return Ok(Some(Refusal::Economics));
        }
    }
    let mut recent = recent_except(
        store,
        &proposal.ad_account,
        day,
        now,
        current.policy.observation_days,
        Some(proposal.id),
    )
    .await?;
    recent.budgets_micros = budgets_micros;
    let Some(from) =
        crate::workspace::history::date_of(day - i64::from(current.policy.cooldown_days))
    else {
        return Ok(Some(Refusal::Cooldown));
    };
    let Some(to) = crate::workspace::history::date_of(day) else {
        return Ok(Some(Refusal::Cooldown));
    };
    for change in store
        .account_changes(&proposal.ad_account, from, to)
        .await?
    {
        if change
            .campaign_id
            .is_some_and(|campaign| campaign != action.campaign_id())
        {
            continue;
        }
        let days = chrono::NaiveDate::parse_from_str(
            change.changed_at.get(..10).unwrap_or_default(),
            "%Y-%m-%d",
        )
        .map(|date| day - crate::workspace::history::day_number(date))
        .unwrap_or(0);
        let entry = recent
            .last_change_days
            .entry(action.campaign_id())
            .or_insert(days);
        *entry = (*entry).min(days);
    }
    Ok(
        match policy::check(&current.policy, current.standing, action, live, &recent) {
            Verdict::Refused(refusal) => Some(refusal),
            Verdict::Allowed(Approval::Required) if automatic => Some(Refusal::Suspended),
            Verdict::Allowed(_) => None,
        },
    )
}
