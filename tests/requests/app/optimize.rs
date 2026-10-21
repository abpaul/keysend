use super::*;
use keysend::{
    optimize::{
        ads::{Fault, Simulated},
        execute::{self, Ran},
        policy::{Live, Policy, Refusal, Standing, AUTOMATIC_KINDS},
        signals::{Kind, Severity, Signal},
        Action, Approval, Draft, Mode, ProposalState,
    },
    settings::Settings,
};
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

const AD: &str = "4829153307";
const CAMPAIGN: i64 = 701;
const BUDGET: i64 = 9001;

async fn user(f: &Fixture, cookie: &Cookie<'static>) -> identity::SignedIn {
    identity::authenticate(&f.ctx.db, cookie.value())
        .await
        .unwrap()
        .unwrap()
}

fn live_settings(f: &Fixture) -> Settings {
    let mut settings = Settings::load(&f.ctx).unwrap();
    settings.optimize.live = true;
    settings.optimize.max_mode = Mode::Autopilot;
    settings
}

fn now(f: &Fixture) -> i64 {
    Settings::load(&f.ctx).unwrap().now().timestamp()
}

/// Today in the Ad Account's time zone (UTC in the fixture), as a day number.
fn day(f: &Fixture) -> i64 {
    keysend::workspace::history::day_number(Settings::load(&f.ctx).unwrap().now().date_naive())
}

fn live(budget: i64) -> Live {
    Live {
        campaign_id: CAMPAIGN,
        status: "ENABLED".into(),
        budget_id: Some(BUDGET),
        budget_micros: Some(budget),
        budget_shared: false,
        budget_campaigns: 1,
        bidding_strategy: "MAXIMIZE_CONVERSIONS".into(),
    }
}

fn cut() -> Action {
    Action::AdjustCampaignBudget {
        campaign_id: CAMPAIGN,
        budget_id: BUDGET,
        from_micros: 80_000_000,
        to_micros: 64_000_000,
    }
}

fn draft(action: Action, approval: Approval, version: i64, now: i64) -> Draft {
    Draft {
        key: format!("{}:{}", action.kind().name(), action.campaign_id()),
        action,
        signal: Signal {
            kind: Kind::CostAboveTarget,
            campaign_id: Some(CAMPAIGN),
            campaign: Some("Search - Real".into()),
            term: None,
            from_day: 0,
            to_day: 29,
            spend: 2400.0,
            clicks: 900.0,
            conversions: 40.0,
            target: Some(25.0),
            chance: Some(0.001),
            lost_to_budget: None,
            daily_spend: Some(80.0),
            daily_budget: Some(80.0),
            data_day: 29,
            severity: Severity::Medium,
            caveats: vec![],
        },
        approval,
        policy_version: version,
        reasons: vec![],
        created_at: now,
        expires_at: now + 86_400,
    }
}

/// An Ad Account in Recommend mode, or in Autopilot with budget cuts allowed.
async fn policy(store: &WorkspaceStore<'_>, mode: Mode, now: i64) -> i64 {
    let mut policy = Policy {
        mode,
        envelope_micros: Some(1_000_000_000),
        max_daily_increase_micros: 50_000_000,
        ..Default::default()
    };
    if mode == Mode::Autopilot {
        policy.automatic = AUTOMATIC_KINDS.into_iter().collect();
    }
    store
        .save_optimization_policy(AD, &policy, Standing::Active, "", now)
        .await
        .unwrap()
}

/// A Proposal that the User approved, as the page submits it.
async fn approved(store: &WorkspaceStore<'_>, action: Action, now: i64) -> i64 {
    let version = store.optimization_policy(AD).await.unwrap().version;
    let draft = draft(action, Approval::Required, version, now);
    let id = store.save_proposal(AD, &draft, now).await.unwrap().unwrap();
    assert!(store
        .approve_proposal(id, &draft.fingerprint(AD), now)
        .await
        .unwrap());
    id
}

async fn state(store: &WorkspaceStore<'_>, id: i64) -> ProposalState {
    store.proposal(id).await.unwrap().unwrap().state
}

#[tokio::test]
#[serial]
async fn an_approved_budget_cut_runs_once_and_is_read_back() {
    let f = Fixture::monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = now(&f);
    policy(&store, Mode::Recommend, now).await;
    let id = approved(&store, cut(), now).await;
    let google = Simulated::new(vec![live(80_000_000)]);
    let settings = live_settings(&f);
    assert_eq!(
        execute::run(&settings, &store, &google, id, day(&f), 80_000_000, now)
            .await
            .unwrap(),
        Ran::Applied
    );
    let quality = store.decision_quality().await.unwrap();
    assert_eq!(
        (
            quality.execution_reservations,
            quality.execution_attempts,
            quality.verified_dispatches
        ),
        (1, 1, 1)
    );
    assert_eq!(
        google.live(CAMPAIGN).unwrap().budget_micros,
        Some(64_000_000)
    );
    assert_eq!(
        google
            .requests()
            .iter()
            .map(|(_, validate)| *validate)
            .collect::<Vec<_>>(),
        [true, false],
        "a change is validated before it is sent"
    );
    // The queue delivers the job again.
    assert_eq!(
        execute::run(&settings, &store, &google, id, day(&f), 80_000_000, now)
            .await
            .unwrap(),
        Ran::Waiting
    );
    assert_eq!(google.sent(), 1);
    let execution = store.execution(id).await.unwrap().unwrap();
    assert!(execution.verified);
    assert_eq!(execution.after.unwrap()["budget_micros"], 64_000_000);
    let kinds = store
        .optimization_events(Some(AD), Some(id), 20)
        .await
        .unwrap()
        .into_iter()
        .rev()
        .map(|event| event.kind)
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        ["proposed", "approved", "applying", "dispatching", "applied"]
    );
    store
        .record_optimization_event(
            AD,
            Some(id),
            "outcome",
            &json!({
                "state":"inconclusive", "days":14, "settled_days":14
            }),
            now,
        )
        .await
        .unwrap();
    assert_eq!(
        store.decision_quality().await.unwrap().observed_executions,
        0,
        "legacy observations without measured coverage cannot establish outcome coverage"
    );
    store
        .record_optimization_event(
            AD,
            Some(id),
            "outcome",
            &json!({
                "state":"inconclusive", "days":14, "settled_days":14, "before_days":14,
                "tracking_reliable":true, "confounding_changes":0
            }),
            now,
        )
        .await
        .unwrap();
    assert_eq!(
        store.decision_quality().await.unwrap().observed_executions,
        1
    );
    store
        .record_optimization_event(
            AD,
            Some(id),
            "outcome",
            &json!({
                "state":"data_quality", "days":14, "settled_days":14, "before_days":14,
                "tracking_reliable":false, "confounding_changes":0
            }),
            now,
        )
        .await
        .unwrap();
    assert_eq!(
        store.decision_quality().await.unwrap().observed_executions,
        0
    );
}

#[tokio::test]
#[serial]
async fn a_live_switch_cannot_escalate_the_installation_beyond_shadow_or_approval_only() {
    let f = Fixture::monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = now(&f);
    policy(&store, Mode::Recommend, now).await;
    let id = approved(&store, cut(), now).await;
    let google = Simulated::new(vec![live(80_000_000)]);
    let mut settings = live_settings(&f);
    settings.optimize.max_mode = Mode::Shadow;
    assert_eq!(
        execute::run(&settings, &store, &google, id, day(&f), 80_000_000, now)
            .await
            .unwrap(),
        Ran::Refused(Refusal::NotLive)
    );
    let version = policy(&store, Mode::Autopilot, now).await;
    let draft = draft(cut(), Approval::Automatic, version, now);
    let id = store.save_proposal(AD, &draft, now).await.unwrap().unwrap();
    assert!(store
        .authorize_proposal(id, &draft.fingerprint(AD), now)
        .await
        .unwrap());
    settings.optimize.max_mode = Mode::Recommend;
    assert_eq!(
        execute::run(&settings, &store, &google, id, day(&f), 80_000_000, now)
            .await
            .unwrap(),
        Ran::Refused(Refusal::NotLive)
    );
    assert!(google.requests().is_empty());
}

#[tokio::test]
#[serial]
async fn an_approval_binds_the_exact_proposal_and_a_proposal_cannot_change_after_it() {
    let f = Fixture::monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = now(&f);
    let version = policy(&store, Mode::Recommend, now).await;
    let shown = draft(cut(), Approval::Required, version, now);
    let id = store.save_proposal(AD, &shown, now).await.unwrap().unwrap();
    let other = draft(
        Action::PauseCampaign {
            campaign_id: CAMPAIGN,
        },
        Approval::Required,
        version,
        now,
    );
    assert!(
        !store
            .approve_proposal(id, &other.fingerprint(AD), now)
            .await
            .unwrap(),
        "an approval of another change approved this one"
    );
    let changed = f
        .ctx
        .db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "UPDATE optimization_proposals SET draft=replace(draft,'64000000','40000000') WHERE id=?",
            vec![id.into()],
        ))
        .await;
    assert!(changed.is_err(), "a Proposal changed after it was made");
    assert!(store
        .approve_proposal(id, &shown.fingerprint(AD), now)
        .await
        .unwrap());
    assert!(
        !store
            .approve_proposal(id, &shown.fingerprint(AD), now)
            .await
            .unwrap(),
        "an approval was replayed"
    );
    let expired = draft(
        Action::PauseCampaign {
            campaign_id: CAMPAIGN,
        },
        Approval::Required,
        version,
        now - 2 * 86_400,
    );
    let late = store
        .save_proposal(AD, &expired, now)
        .await
        .unwrap()
        .unwrap();
    assert!(
        !store
            .approve_proposal(late, &expired.fingerprint(AD), now)
            .await
            .unwrap(),
        "an expired Proposal was approved"
    );
}

#[tokio::test]
#[serial]
async fn another_workspace_cannot_see_approve_or_run_a_proposal() {
    let f = Fixture::monitoring().await;
    let owner = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &owner.workspace);
    let now = now(&f);
    let version = policy(&store, Mode::Recommend, now).await;
    let shown = draft(cut(), Approval::Required, version, now);
    let id = store.save_proposal(AD, &shown, now).await.unwrap().unwrap();
    let other = user(&f, &f.b).await;
    let theirs = WorkspaceStore::new(&f.ctx.db, &other.workspace);
    assert!(theirs.proposal(id).await.unwrap().is_none());
    assert!(!theirs
        .approve_proposal(id, &shown.fingerprint(AD), now)
        .await
        .unwrap());
    assert!(theirs
        .save_proposal(AD, &shown, now)
        .await
        .unwrap()
        .is_none());
    assert!(theirs
        .save_optimization_policy(AD, &Policy::default(), Standing::Active, "", now)
        .await
        .is_err());
    for path in [
        format!("/optimize/proposals/{id}"),
        format!("/ad-accounts/{AD}/optimize"),
    ] {
        let response = f.request.get(&path).add_cookie(f.b.clone()).await;
        assert_eq!(response.status_code(), 404, "{path}");
    }
    let response = f
        .request
        .post(&format!("/optimize/proposals/{id}/approve"))
        .add_cookie(f.b.clone())
        .add_header("origin", ORIGIN)
        .form(&[("fingerprint", shown.fingerprint(AD))])
        .await;
    assert_eq!(response.status_code(), 404);
    assert_eq!(state(&store, id).await, ProposalState::Open);
    let google = Simulated::new(vec![live(80_000_000)]);
    assert_eq!(
        execute::run(&live_settings(&f), &theirs, &google, id, day(&f), 0, now)
            .await
            .unwrap(),
        Ran::Waiting
    );
    assert!(google.requests().is_empty());
}

#[tokio::test]
#[serial]
async fn a_budget_that_someone_changed_since_the_proposal_is_not_overwritten() {
    let f = Fixture::monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = now(&f);
    policy(&store, Mode::Recommend, now).await;
    let id = approved(&store, cut(), now).await;
    let google = Simulated::new(vec![live(90_000_000)]);
    assert_eq!(
        execute::run(&live_settings(&f), &store, &google, id, day(&f), 0, now)
            .await
            .unwrap(),
        Ran::Refused(Refusal::Stale)
    );
    assert!(google.requests().is_empty());
    assert_eq!(
        google.live(CAMPAIGN).unwrap().budget_micros,
        Some(90_000_000)
    );
    assert_eq!(state(&store, id).await, ProposalState::Conflict);
}

#[tokio::test]
#[serial]
async fn a_shared_budget_is_never_changed() {
    let f = Fixture::monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = now(&f);
    policy(&store, Mode::Recommend, now).await;
    let id = approved(&store, cut(), now).await;
    let other = Live {
        campaign_id: 702,
        ..live(80_000_000)
    };
    let google = Simulated::new(vec![
        Live {
            budget_shared: true,
            budget_campaigns: 2,
            ..live(80_000_000)
        },
        other,
    ]);
    assert_eq!(
        execute::run(&live_settings(&f), &store, &google, id, day(&f), 0, now)
            .await
            .unwrap(),
        Ran::Refused(Refusal::SharedBudget)
    );
    assert!(google.requests().is_empty());
    assert_eq!(google.live(702).unwrap().budget_micros, Some(80_000_000));
}

#[tokio::test]
#[serial]
async fn a_lost_answer_is_settled_by_reading_the_budget_and_the_change_is_not_sent_twice() {
    let f = Fixture::monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = now(&f);
    policy(&store, Mode::Recommend, now).await;
    let id = approved(&store, cut(), now).await;
    let google = Simulated::new(vec![live(80_000_000)]);
    google.fault(Fault::LostAnswer);
    assert_eq!(
        execute::run(&live_settings(&f), &store, &google, id, day(&f), 0, now)
            .await
            .unwrap(),
        Ran::Applied
    );
    assert_eq!(google.sent(), 1);
    assert_eq!(state(&store, id).await, ProposalState::Applied);
}

#[tokio::test]
#[serial]
async fn an_old_budget_after_a_timeout_stays_uncertain_and_is_never_sent_again() {
    let f = Fixture::monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = now(&f);
    policy(&store, Mode::Recommend, now).await;
    let id = approved(&store, cut(), now).await;
    let google = Simulated::new(vec![live(80_000_000)]);
    google.fault(Fault::Timeout);
    assert_eq!(
        execute::run(&live_settings(&f), &store, &google, id, day(&f), 0, now)
            .await
            .unwrap(),
        Ran::Uncertain
    );
    assert_eq!(google.sent(), 1);
    assert_eq!(
        google.live(CAMPAIGN).unwrap().budget_micros,
        Some(80_000_000)
    );
    assert_eq!(
        execute::reconcile(&store, &google, id, now + 700)
            .await
            .unwrap(),
        Ran::Uncertain
    );
    assert_eq!(state(&store, id).await, ProposalState::Uncertain);
    assert_eq!(google.sent(), 1);
}

#[tokio::test]
#[serial]
async fn an_uncertain_change_is_reconciled_by_a_read_without_being_sent_again() {
    let f = Fixture::monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = now(&f);
    policy(&store, Mode::Recommend, now).await;
    let id = approved(&store, cut(), now).await;
    let google = Simulated::new(vec![live(80_000_000)]);
    google.fault(Fault::LostAnswer);
    google.fault(Fault::ReadUnknown);
    assert_eq!(
        execute::run(&live_settings(&f), &store, &google, id, day(&f), 0, now)
            .await
            .unwrap(),
        Ran::Uncertain
    );
    assert_eq!(state(&store, id).await, ProposalState::Uncertain);
    // A change of another Campaign of the Ad Account waits while this one is uncertain.
    let pause = approved(&store, Action::PauseCampaign { campaign_id: 702 }, now).await;
    assert_eq!(
        execute::run(&live_settings(&f), &store, &google, pause, day(&f), 0, now)
            .await
            .unwrap(),
        Ran::Waiting
    );
    assert_eq!(
        execute::reconcile(&store, &google, id, now + 700)
            .await
            .unwrap(),
        Ran::Applied
    );
    assert_eq!(google.sent(), 1);
    assert_eq!(state(&store, id).await, ProposalState::Applied);
}

#[tokio::test]
#[serial]
async fn a_refused_validation_changes_nothing() {
    let f = Fixture::monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = now(&f);
    policy(&store, Mode::Recommend, now).await;
    let id = approved(&store, cut(), now).await;
    let google = Simulated::new(vec![live(80_000_000)]);
    google.fault(Fault::Refuse("BUDGET_BELOW_PER_DAY_MINIMUM"));
    let ran = execute::run(&live_settings(&f), &store, &google, id, day(&f), 0, now)
        .await
        .unwrap();
    assert!(matches!(ran, Ran::Failed(_)), "{ran:?}");
    assert_eq!(google.sent(), 0);
    assert_eq!(
        store.decision_quality().await.unwrap().execution_attempts,
        0
    );
    assert_eq!(
        google.live(CAMPAIGN).unwrap().budget_micros,
        Some(80_000_000)
    );
    assert!(store
        .execution(id)
        .await
        .unwrap()
        .unwrap()
        .error
        .unwrap()
        .contains("BUDGET_BELOW_PER_DAY_MINIMUM"));
}

#[tokio::test]
#[serial]
async fn a_revoked_google_connection_fails_the_change_and_suspends_the_autopilot() {
    let f = Fixture::monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = now(&f);
    policy(&store, Mode::Autopilot, now).await;
    let id = approved(&store, cut(), now).await;
    let google = Simulated::new(vec![live(80_000_000)]);
    google.fault(Fault::Revoke);
    let ran = execute::run(&live_settings(&f), &store, &google, id, day(&f), 0, now)
        .await
        .unwrap();
    assert_eq!(ran, Ran::Failed(keysend::optimize::ads::AdsError::Access));
    let current = store.optimization_policy(AD).await.unwrap();
    assert_eq!(current.standing, Standing::Suspended);
    assert_eq!(current.reason, "access");
}

#[tokio::test]
#[serial]
async fn the_workspace_stop_refuses_approved_changes_until_the_user_resumes() {
    let f = Fixture::monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = now(&f);
    policy(&store, Mode::Recommend, now).await;
    let id = approved(&store, cut(), now).await;
    store.set_optimization_stopped(true, now).await.unwrap();
    assert_eq!(state(&store, id).await, ProposalState::Failed);
    let google = Simulated::new(vec![live(80_000_000)]);
    let later = approved(
        &store,
        Action::PauseCampaign {
            campaign_id: CAMPAIGN,
        },
        now,
    )
    .await;
    assert_eq!(
        execute::run(&live_settings(&f), &store, &google, later, day(&f), 0, now)
            .await
            .unwrap(),
        Ran::Refused(Refusal::Stopped)
    );
    assert!(google.requests().is_empty());
    store.set_optimization_stopped(false, now).await.unwrap();
    assert!(!store.optimization_stopped().await.unwrap());
}

#[tokio::test]
#[serial]
async fn a_change_the_autopilot_authorized_does_not_run_after_the_user_switches_it_off() {
    let f = Fixture::monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = now(&f);
    let version = policy(&store, Mode::Autopilot, now).await;
    let automatic = draft(cut(), Approval::Automatic, version, now);
    let id = store
        .save_proposal(AD, &automatic, now)
        .await
        .unwrap()
        .unwrap();
    assert!(store
        .authorize_proposal(id, &automatic.fingerprint(AD), now)
        .await
        .unwrap());
    let current = store.optimization_policy(AD).await.unwrap();
    store
        .save_optimization_policy(AD, &current.policy, Standing::Revoked, "user", now)
        .await
        .unwrap();
    let google = Simulated::new(vec![live(80_000_000)]);
    assert_eq!(
        execute::run(&live_settings(&f), &store, &google, id, day(&f), 0, now)
            .await
            .unwrap(),
        Ran::Refused(Refusal::PolicyChanged)
    );
    assert!(google.requests().is_empty());
    // A Proposal that needs the User cannot be authorized by the Autopilot.
    let pause = draft(
        Action::PauseCampaign {
            campaign_id: CAMPAIGN,
        },
        Approval::Required,
        version,
        now,
    );
    let pause = store.save_proposal(AD, &pause, now).await.unwrap().unwrap();
    let fingerprint = store.proposal(pause).await.unwrap().unwrap().fingerprint;
    assert!(!store
        .authorize_proposal(pause, &fingerprint, now)
        .await
        .unwrap());
}

#[tokio::test]
#[serial]
async fn two_workers_cannot_pass_the_daily_change_limit_together() {
    let f = Fixture::monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = now(&f);
    let policy = Policy {
        mode: Mode::Recommend,
        changes_per_day: 1,
        ..Default::default()
    };
    store
        .save_optimization_policy(AD, &policy, Standing::Active, "", now)
        .await
        .unwrap();
    let first = approved(&store, cut(), now).await;
    let second = approved(
        &store,
        Action::AdjustCampaignBudget {
            campaign_id: 702,
            budget_id: 9002,
            from_micros: 50_000_000,
            to_micros: 40_000_000,
        },
        now,
    )
    .await;
    let google = Simulated::new(vec![
        live(80_000_000),
        Live {
            campaign_id: 702,
            budget_id: Some(9002),
            ..live(50_000_000)
        },
    ]);
    let settings = live_settings(&f);
    let (a, b) = tokio::join!(
        execute::run(&settings, &store, &google, first, day(&f), 0, now),
        execute::run(&settings, &store, &google, second, day(&f), 0, now),
    );
    let ran = [a.unwrap(), b.unwrap()];
    assert_eq!(
        ran.iter().filter(|ran| **ran == Ran::Applied).count(),
        1,
        "{ran:?}"
    );
    assert_eq!(google.sent(), 1);
    for id in [first, second] {
        if state(&store, id).await == ProposalState::Approved {
            assert_eq!(
                execute::run(&settings, &store, &google, id, day(&f), 0, now)
                    .await
                    .unwrap(),
                Ran::Refused(Refusal::DailyChanges)
            );
        }
    }
    assert_eq!(google.sent(), 1);
}

#[tokio::test]
#[serial]
async fn applying_is_refused_while_the_installation_is_not_live() {
    let f = Fixture::monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = now(&f);
    policy(&store, Mode::Recommend, now).await;
    let id = approved(&store, cut(), now).await;
    let settings = Settings::load(&f.ctx).unwrap();
    assert!(!settings.optimize.live, "applying is on by default");
    let google = Simulated::new(vec![live(80_000_000)]);
    assert_eq!(
        execute::run(&settings, &store, &google, id, day(&f), 0, now)
            .await
            .unwrap(),
        Ran::Refused(Refusal::NotLive)
    );
    let negative = approved(
        &store,
        Action::AddNegativeKeyword {
            campaign_id: CAMPAIGN,
            text: "free".into(),
            match_type: keysend::optimize::MatchType::Exact,
        },
        now,
    )
    .await;
    assert_eq!(
        execute::run(
            &live_settings(&f),
            &store,
            &google,
            negative,
            day(&f),
            0,
            now
        )
        .await
        .unwrap(),
        Ran::Refused(Refusal::NotLive),
        "a kind that is not live ran"
    );
    assert!(google.requests().is_empty());
}

#[tokio::test]
#[serial]
async fn the_optimization_history_is_append_only() {
    let f = Fixture::monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = now(&f);
    policy(&store, Mode::Recommend, now).await;
    for sql in [
        "UPDATE optimization_events SET kind='applied'",
        "UPDATE optimization_policies SET standing='active'",
    ] {
        assert!(
            f.ctx
                .db
                .execute_raw(Statement::from_string(DatabaseBackend::Sqlite, sql))
                .await
                .is_err(),
            "{sql}"
        );
    }
}

#[tokio::test]
#[serial]
async fn a_budget_change_reaches_google_ads_as_one_validated_operation_with_an_exact_field_mask() {
    let f = Fixture::monitoring().await;
    f.prepare_monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = now(&f);
    policy(&store, Mode::Recommend, now).await;
    let id = approved(&store, cut(), now).await;
    f.calls.lock().unwrap().google.live_budget = Some(80_000_000);
    let ran = keysend::optimize::cycle::execute_one(&live_settings(&f), &store, id, now)
        .await
        .unwrap();
    assert_eq!(ran, Ran::Applied);
    {
        let calls = f.calls.lock().unwrap();
        assert_eq!(calls.google.live_budget, Some(64_000_000));
        let sent = &calls.google.mutations;
        assert_eq!(sent.len(), 2, "one validation and one change");
        for (index, (service, headers, body)) in sent.iter().enumerate() {
            assert_eq!(service, "campaignBudgets");
            assert_eq!(body["validateOnly"], index == 0);
            assert_eq!(body["partialFailure"], false);
            assert_eq!(body["responseContentType"], "MUTABLE_RESOURCE");
            assert_eq!(
                body["operations"],
                json!([{"update":{"resourceName":"customers/4829153307/campaignBudgets/9001","amountMicros":"64000000"},"updateMask":"amountMicros"}])
            );
            assert_eq!(headers["authorization"], "Bearer google-access-secret");
            assert!(headers.get("developer-token").is_none());
        }
    }
    let execution = store.execution(id).await.unwrap().unwrap();
    assert_eq!(execution.request_id.as_deref(), Some("stub-request-1"));
    assert!(execution.verified);
}

#[tokio::test]
#[serial]
async fn a_change_without_an_answer_from_google_ads_is_never_sent_again() {
    let f = Fixture::monitoring().await;
    f.prepare_monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = now(&f);
    policy(&store, Mode::Recommend, now).await;
    let id = approved(&store, cut(), now).await;
    {
        let mut calls = f.calls.lock().unwrap();
        calls.google.live_budget = Some(80_000_000);
        calls.google.mutate_failure = Some(axum::http::StatusCode::SERVICE_UNAVAILABLE);
    }
    let ran = keysend::optimize::cycle::execute_one(&live_settings(&f), &store, id, now)
        .await
        .unwrap();
    assert_eq!(ran, Ran::Uncertain);
    let calls = f.calls.lock().unwrap();
    assert_eq!(calls.google.live_budget, Some(80_000_000));
    let searches = calls
        .google
        .requests
        .iter()
        .filter(|(_, body)| {
            body["query"]
                .as_str()
                .unwrap()
                .contains("WHERE campaign.id = 701")
        })
        .count();
    assert_eq!(
        searches, 3,
        "read before validation, before sending, and after the lost answer"
    );
    assert_eq!(calls.google.mutations.len(), 2);
}

#[tokio::test]
#[serial]
async fn the_daily_run_evaluates_an_ad_account_in_recommend_mode_and_an_evaluation_without_new_data_does_nothing(
) {
    let f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = now(&f);
    let policy = Policy {
        mode: Mode::Recommend,
        target_cost_micros: Some(5_000_000),
        ..Default::default()
    };
    store
        .save_optimization_policy(AD, &policy, Standing::Active, "user", now)
        .await
        .unwrap();
    // The daily run collects the budget of each Campaign with the Campaigns.
    f.calls.lock().unwrap().google.monitoring_campaigns = Some(vec![
        json!({"campaign":{"id":"701","name":"Search - Real","status":"ENABLED","advertisingChannelType":"SEARCH","biddingStrategyType":"MAXIMIZE_CONVERSIONS"},"campaignBudget":{"id":"9001","amountMicros":"80000000","explicitlyShared":false,"referenceCount":"1"}}),
    ]);
    assert!(store.optimization_evaluation(AD).await.unwrap().is_none());
    keysend::monitoring::worker::run(&f.ctx, &job)
        .await
        .unwrap();
    assert_eq!(
        store.campaign_budgets(AD).await.unwrap()[&CAMPAIGN].budget_id,
        BUDGET
    );
    let (_, evaluated) = store
        .optimization_evaluation(AD)
        .await
        .unwrap()
        .expect("the daily run evaluated the Ad Account");
    // The Campaign spends 10 a day of an 80 budget: a cut would change nothing.
    assert_eq!(
        store.optimization_holds(AD).await.unwrap()["budget_not_binding"],
        1
    );
    assert!(store
        .proposals(Some(AD), &ProposalState::ALL, 10)
        .await
        .unwrap()
        .is_empty());
    let requests = f.calls.lock().unwrap().google.requests.len();
    let later = keysend::optimize::cycle::evaluate(&live_settings(&f), &store, AD, now + 3600)
        .await
        .unwrap();
    assert!(later.is_empty());
    assert_eq!(
        store.optimization_evaluation(AD).await.unwrap().unwrap().1,
        evaluated,
        "an evaluation without new data ran again"
    );
    assert_eq!(f.calls.lock().unwrap().google.requests.len(), requests);
}

#[tokio::test]
#[serial]
async fn an_approved_change_runs_after_the_change_it_waited_for_is_settled() {
    let f = Fixture::monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = now(&f);
    policy(&store, Mode::Recommend, now).await;
    let first = approved(&store, cut(), now).await;
    let google = Simulated::new(vec![
        live(80_000_000),
        Live {
            campaign_id: 702,
            ..live(50_000_000)
        },
    ]);
    google.fault(Fault::LostAnswer);
    google.fault(Fault::ReadUnknown);
    let settings = live_settings(&f);
    assert_eq!(
        execute::run(&settings, &store, &google, first, day(&f), 0, now)
            .await
            .unwrap(),
        Ran::Uncertain
    );
    let second = approved(&store, Action::PauseCampaign { campaign_id: 702 }, now).await;
    assert_eq!(
        execute::run(&settings, &store, &google, second, day(&f), 0, now)
            .await
            .unwrap(),
        Ran::Waiting
    );
    assert!(execute::waiting(&store).await.unwrap().is_empty());
    execute::reconcile(&store, &google, first, now + 700)
        .await
        .unwrap();
    assert_eq!(execute::waiting(&store).await.unwrap(), [second]);
    assert_eq!(
        execute::run(&settings, &store, &google, second, day(&f), 0, now + 700)
            .await
            .unwrap(),
        Ran::Applied
    );
    assert_eq!(google.live(702).unwrap().status, "PAUSED");
}

#[tokio::test]
#[serial]
async fn raises_are_refused_without_a_current_account_wide_budget_total() {
    let f = Fixture::monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = now(&f);
    let policy = Policy {
        mode: Mode::Recommend,
        envelope_micros: Some(95_000_000),
        max_daily_increase_micros: 50_000_000,
        ..Default::default()
    };
    store
        .save_optimization_policy(AD, &policy, Standing::Active, "", now)
        .await
        .unwrap();
    let raise = |campaign: i64| Action::AdjustCampaignBudget {
        campaign_id: campaign,
        budget_id: campaign + 8300,
        from_micros: 45_000_000,
        to_micros: 49_500_000,
    };
    let first = approved(&store, raise(CAMPAIGN), now).await;
    let second = approved(&store, raise(702), now).await;
    let google = Simulated::new(
        [CAMPAIGN, 702]
            .map(|campaign| Live {
                campaign_id: campaign,
                budget_id: Some(campaign + 8300),
                ..live(45_000_000)
            })
            .to_vec(),
    );
    let settings = live_settings(&f);
    let (a, b) = tokio::join!(
        execute::run(&settings, &store, &google, first, day(&f), 90_000_000, now),
        execute::run(&settings, &store, &google, second, day(&f), 90_000_000, now),
    );
    assert_eq!(a.unwrap(), Ran::Refused(Refusal::BudgetTotalUnknown));
    assert_eq!(b.unwrap(), Ran::Refused(Refusal::BudgetTotalUnknown));
    let tomorrow = now + 86_400;
    let third = approved(&store, raise(CAMPAIGN), tomorrow).await;
    assert_eq!(
        execute::run(
            &settings,
            &store,
            &google,
            third,
            day(&f) + 1,
            90_000_000,
            tomorrow
        )
        .await
        .unwrap(),
        Ran::Refused(Refusal::BudgetTotalUnknown)
    );
    assert!(google.requests().is_empty());
}

/// A Google Ads in which the User presses Stop while KeySend reads the live value.
struct StopWhileReading<'a> {
    store: &'a WorkspaceStore<'a>,
    google: Simulated,
    now: i64,
}

#[async_trait::async_trait]
impl keysend::optimize::ads::Ads for StopWhileReading<'_> {
    async fn campaign(
        &self,
        campaign_id: i64,
    ) -> Result<Option<Live>, keysend::optimize::ads::AdsError> {
        if !self.store.optimization_stopped().await.unwrap() {
            self.store
                .set_optimization_stopped(true, self.now)
                .await
                .unwrap();
        }
        self.google.campaign(campaign_id).await
    }

    async fn apply(
        &self,
        action: &Action,
        validate_only: bool,
    ) -> Result<keysend::optimize::ads::Applied, keysend::optimize::ads::AdsError> {
        self.google.apply(action, validate_only).await
    }
}

#[tokio::test]
#[serial]
async fn a_stop_pressed_while_a_change_is_read_prevents_the_change() {
    let f = Fixture::monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = now(&f);
    policy(&store, Mode::Recommend, now).await;
    let id = approved(&store, cut(), now).await;
    let stopping = StopWhileReading {
        store: &store,
        google: Simulated::new(vec![live(80_000_000)]),
        now,
    };
    assert_eq!(
        execute::run(&live_settings(&f), &store, &stopping, id, day(&f), 0, now)
            .await
            .unwrap(),
        Ran::Refused(Refusal::Stopped)
    );
    assert_eq!(stopping.google.sent(), 0);
    assert_eq!(state(&store, id).await, ProposalState::Failed);
}

#[tokio::test]
#[serial]
async fn protecting_a_campaign_keeps_a_suspended_autopilot_suspended() {
    let f = Fixture::monitoring().await;
    f.prepare_monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = now(&f);
    policy(&store, Mode::Autopilot, now).await;
    execute::suspend(&store, AD, "spend", now).await.unwrap();
    let form = [
        ("mode", "autopilot"),
        ("automatic", "adjust_campaign_budget"),
        ("protected", "701"),
        ("max_decrease_percent", "20"),
        ("max_increase_percent", "10"),
        ("envelope", "1000"),
        ("max_daily_increase", "50"),
        ("changes_per_day", "3"),
        ("cooldown_days", "7"),
        ("min_conversions", "10"),
        ("target_cost", ""),
    ];
    let response = f
        .request
        .post(&format!("/ad-accounts/{AD}/optimize"))
        .add_cookie(f.a.clone())
        .add_header("origin", ORIGIN)
        .form(&form)
        .await;
    assert_eq!(response.status_code(), 303);
    let current = store.optimization_policy(AD).await.unwrap();
    assert!(current.policy.protected.contains(&CAMPAIGN));
    assert_eq!(current.standing, Standing::Suspended);
    let unknown = [("protected", "999"), ("mode", "recommend")];
    let response = f
        .request
        .post(&format!("/ad-accounts/{AD}/optimize"))
        .add_cookie(f.a.clone())
        .add_header("origin", ORIGIN)
        .form(
            &form
                .iter()
                .filter(|(name, _)| *name != "protected" && *name != "mode")
                .copied()
                .chain(unknown)
                .collect::<Vec<_>>(),
        )
        .await;
    assert_eq!(
        response.status_code(),
        422,
        "an unknown Campaign was dropped silently"
    );
}

#[tokio::test]
#[serial]
async fn a_duplicate_delivery_cannot_fail_another_workers_reservation() {
    let f = Fixture::monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = now(&f);
    policy(&store, Mode::Recommend, now).await;
    let id = approved(&store, cut(), now).await;
    let stale = store.proposal(id).await.unwrap().unwrap();
    assert_eq!(
        store
            .claim_execution(&stale, day(&f), false, 0, 3, 0, now)
            .await
            .unwrap(),
        keysend::workspace::optimization::Claim::Claimed
    );
    assert!(!store
        .set_proposal_state(id, ProposalState::Failed, json!({"refusal":"stopped"}), now)
        .await
        .unwrap());
    assert_eq!(state(&store, id).await, ProposalState::Applying);
}

enum DuringValidation {
    Stop,
    DailyLimit,
    Protect,
    Limit,
    Access,
    Shadow,
    Manual,
    Revoke,
    Audit,
    Budget,
    Reconcile(i64),
    ReconcileThenAccess(i64),
}

struct ChangesDuringValidation<'a> {
    store: &'a WorkspaceStore<'a>,
    db: &'a sea_orm::DatabaseConnection,
    google: Simulated,
    change: DuringValidation,
    now: i64,
}

#[async_trait::async_trait]
impl keysend::optimize::ads::Ads for ChangesDuringValidation<'_> {
    async fn campaign(&self, id: i64) -> Result<Option<Live>, keysend::optimize::ads::AdsError> {
        if matches!(self.change, DuringValidation::Revoke) && !self.google.requests().is_empty() {
            return Err(keysend::optimize::ads::AdsError::Access);
        }
        self.google.campaign(id).await
    }
    async fn apply(
        &self,
        action: &Action,
        validate: bool,
    ) -> Result<keysend::optimize::ads::Applied, keysend::optimize::ads::AdsError> {
        if !validate {
            if let DuringValidation::ReconcileThenAccess(id) = self.change {
                execute::reconcile(self.store, &self.google, id, self.now + 700)
                    .await
                    .unwrap();
                return Err(keysend::optimize::ads::AdsError::Access);
            }
        }
        let result = self.google.apply(action, validate).await;
        if validate {
            let mut current = self.store.optimization_policy(AD).await.unwrap();
            match self.change {
                DuringValidation::Stop => {
                    self.store
                        .set_optimization_stopped(true, self.now)
                        .await
                        .unwrap();
                }
                DuringValidation::DailyLimit => current.policy.changes_per_day = 1,
                DuringValidation::Protect => {
                    current.policy.protected.insert(CAMPAIGN);
                }
                DuringValidation::Limit => current.policy.max_decrease_percent = 10,
                DuringValidation::Shadow => current.policy.mode = Mode::Shadow,
                DuringValidation::Manual => current.policy.automatic.clear(),
                DuringValidation::Access => {
                    self.db
                        .execute_raw(Statement::from_sql_and_values(
                            DatabaseBackend::Sqlite,
                            "UPDATE workspace_trials SET started_at=?",
                            [(self.now - 30 * 86_400).into()],
                        ))
                        .await
                        .unwrap();
                }
                DuringValidation::Revoke
                | DuringValidation::Audit
                | DuringValidation::ReconcileThenAccess(_) => {}
                DuringValidation::Budget => self.google.set(live(72_000_000)),
                DuringValidation::Reconcile(id) => {
                    execute::reconcile(self.store, &self.google, id, self.now + 700)
                        .await
                        .unwrap();
                }
            }
            if matches!(
                self.change,
                DuringValidation::Protect
                    | DuringValidation::DailyLimit
                    | DuringValidation::Limit
                    | DuringValidation::Shadow
                    | DuringValidation::Manual
            ) {
                self.store
                    .save_optimization_policy(AD, &current.policy, current.standing, "", self.now)
                    .await
                    .unwrap();
            }
        }
        if !validate && matches!(self.change, DuringValidation::Audit) {
            self.db.execute_unprepared("CREATE TRIGGER fail_execution_audit BEFORE INSERT ON optimization_events WHEN NEW.kind='applied' BEGIN SELECT RAISE(ABORT,'injected audit failure'); END").await.unwrap();
        }
        result
    }
}

#[tokio::test]
#[serial]
async fn the_current_policy_and_access_are_checked_after_validation() {
    for (change, refusal) in [
        (DuringValidation::Stop, Refusal::Stopped),
        (DuringValidation::Protect, Refusal::NotEligible),
        (DuringValidation::Limit, Refusal::StepTooLarge),
        (DuringValidation::Access, Refusal::Access),
        (DuringValidation::Shadow, Refusal::Shadow),
        (DuringValidation::Budget, Refusal::Stale),
    ] {
        let f = Fixture::monitoring().await;
        let signed = user(&f, &f.a).await;
        let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
        let now = now(&f);
        policy(&store, Mode::Recommend, now).await;
        let id = approved(&store, cut(), now).await;
        let google = ChangesDuringValidation {
            store: &store,
            db: &f.ctx.db,
            google: Simulated::new(vec![live(80_000_000)]),
            change,
            now,
        };
        assert_eq!(
            execute::run(&live_settings(&f), &store, &google, id, day(&f), 0, now)
                .await
                .unwrap(),
            Ran::Refused(refusal)
        );
        assert_eq!(google.google.sent(), 0);
    }
}

#[tokio::test]
#[serial]
async fn reconciliation_revokes_a_workers_permission_to_send() {
    let f = Fixture::monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = now(&f);
    policy(&store, Mode::Recommend, now).await;
    let id = approved(&store, cut(), now).await;
    let google = ChangesDuringValidation {
        store: &store,
        db: &f.ctx.db,
        google: Simulated::new(vec![live(80_000_000)]),
        change: DuringValidation::Reconcile(id),
        now,
    };
    assert_eq!(
        execute::run(&live_settings(&f), &store, &google, id, day(&f), 0, now)
            .await
            .unwrap(),
        Ran::Waiting
    );
    assert_eq!(google.google.sent(), 0);
    assert_eq!(state(&store, id).await, ProposalState::Uncertain);
}

#[tokio::test]
#[serial]
async fn an_external_campaign_or_account_change_holds_the_next_execution() {
    for campaign in [Some(CAMPAIGN), None] {
        let f = Fixture::monitoring().await;
        let signed = user(&f, &f.a).await;
        let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
        let now = now(&f);
        policy(&store, Mode::Recommend, now).await;
        let id = approved(&store, cut(), now).await;
        let date = Settings::load(&f.ctx).unwrap().now().date_naive();
        store
            .save_history(
                AD,
                date,
                date,
                &[],
                &[],
                &[keysend::workspace::history::AccountChange {
                    changed_at: format!("{date} 00:00:00"),
                    resource_name: "external-change".into(),
                    resource_type: "CAMPAIGN".into(),
                    operation: "UPDATE".into(),
                    campaign_id: campaign,
                    changed_fields: "status".into(),
                    old_value: "{}".into(),
                    new_value: "{}".into(),
                    change_resource_name: None,
                }],
            )
            .await
            .unwrap();
        let google = Simulated::new(vec![live(80_000_000)]);
        assert_eq!(
            execute::run(&live_settings(&f), &store, &google, id, day(&f), 0, now)
                .await
                .unwrap(),
            Ran::Refused(Refusal::Cooldown)
        );
        assert!(google.requests().is_empty());
    }
}

struct PausedAtValidation {
    google: Simulated,
    validated: tokio::sync::Notify,
    resume: tokio::sync::Notify,
}

#[async_trait::async_trait]
impl keysend::optimize::ads::Ads for PausedAtValidation {
    async fn campaign(&self, id: i64) -> Result<Option<Live>, keysend::optimize::ads::AdsError> {
        self.google.campaign(id).await
    }
    async fn apply(
        &self,
        action: &Action,
        validate: bool,
    ) -> Result<keysend::optimize::ads::Applied, keysend::optimize::ads::AdsError> {
        let result = self.google.apply(action, validate).await;
        if validate {
            self.validated.notify_one();
            self.resume.notified().await;
        }
        result
    }
}

#[tokio::test]
#[serial]
async fn two_workers_delivering_the_same_proposal_send_only_once() {
    let f = Fixture::monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = now(&f);
    policy(&store, Mode::Recommend, now).await;
    let id = approved(&store, cut(), now).await;
    let google = PausedAtValidation {
        google: Simulated::new(vec![live(80_000_000)]),
        validated: Default::default(),
        resume: Default::default(),
    };
    let settings = live_settings(&f);
    let first = execute::run(&settings, &store, &google, id, day(&f), 0, now);
    let second = async {
        google.validated.notified().await;
        let ran = execute::run(&settings, &store, &google, id, day(&f), 0, now)
            .await
            .unwrap();
        assert_eq!(state(&store, id).await, ProposalState::Applying);
        google.resume.notify_one();
        ran
    };
    let (first, second) = tokio::join!(first, second);
    assert_eq!(first.unwrap(), Ran::Applied);
    assert_eq!(second, Ran::Waiting);
    assert_eq!(google.google.sent(), 1);
}

#[tokio::test]
#[serial]
async fn an_interrupted_worker_is_reconciled_without_resending() {
    let f = Fixture::monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = now(&f);
    policy(&store, Mode::Recommend, now).await;
    let id = approved(&store, cut(), now).await;
    let google = PausedAtValidation {
        google: Simulated::new(vec![live(80_000_000)]),
        validated: Default::default(),
        resume: Default::default(),
    };
    let settings = live_settings(&f);
    tokio::select! {
        _ = google.validated.notified() => {},
        result = execute::run(&settings, &store, &google, id, day(&f), 0, now) => panic!("worker finished before interruption: {result:?}"),
    }
    assert_eq!(state(&store, id).await, ProposalState::Applying);
    assert_eq!(
        execute::reconcile(&store, &google, id, now + 700)
            .await
            .unwrap(),
        Ran::Uncertain
    );
    assert_eq!(
        execute::run(&settings, &store, &google, id, day(&f), 0, now + 700)
            .await
            .unwrap(),
        Ran::Waiting
    );
    assert_eq!(google.google.sent(), 0);
    google.google.set(live(64_000_000));
    assert_eq!(
        execute::reconcile(&store, &google, id, now + 800)
            .await
            .unwrap(),
        Ran::Applied
    );
    assert_eq!(google.google.sent(), 0);
}

#[tokio::test]
#[serial]
async fn a_connection_revoked_after_validation_prevents_the_write() {
    let f = Fixture::monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = now(&f);
    policy(&store, Mode::Recommend, now).await;
    let id = approved(&store, cut(), now).await;
    let google = ChangesDuringValidation {
        store: &store,
        db: &f.ctx.db,
        google: Simulated::new(vec![live(80_000_000)]),
        change: DuringValidation::Revoke,
        now,
    };
    assert_eq!(
        execute::run(&live_settings(&f), &store, &google, id, day(&f), 0, now)
            .await
            .unwrap(),
        Ran::Failed(keysend::optimize::ads::AdsError::Access)
    );
    assert_eq!(google.google.sent(), 0);
}

#[tokio::test]
#[serial]
async fn an_audit_failure_after_the_write_is_recoverable_without_a_duplicate() {
    let f = Fixture::monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = now(&f);
    policy(&store, Mode::Recommend, now).await;
    let id = approved(&store, cut(), now).await;
    let google = ChangesDuringValidation {
        store: &store,
        db: &f.ctx.db,
        google: Simulated::new(vec![live(80_000_000)]),
        change: DuringValidation::Audit,
        now,
    };
    let settings = live_settings(&f);
    assert!(
        execute::run(&settings, &store, &google, id, day(&f), 0, now)
            .await
            .unwrap_err()
            .to_string()
            .contains("injected audit failure")
    );
    assert_eq!(state(&store, id).await, ProposalState::Applying);
    assert_eq!(google.google.sent(), 1);
    assert_eq!(
        execute::run(&settings, &store, &google, id, day(&f), 0, now)
            .await
            .unwrap(),
        Ran::Waiting
    );
    f.ctx
        .db
        .execute_unprepared("DROP TRIGGER fail_execution_audit")
        .await
        .unwrap();
    assert_eq!(
        execute::reconcile(&store, &google, id, now + 700)
            .await
            .unwrap(),
        Ran::Applied
    );
    assert!(store.execution(id).await.unwrap().unwrap().verified);
    assert_eq!(google.google.sent(), 1);
}

#[tokio::test]
#[serial]
async fn an_automatic_change_waits_when_the_user_requires_manual_approval_during_validation() {
    let f = Fixture::monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = now(&f);
    let version = policy(&store, Mode::Autopilot, now).await;
    let draft = draft(cut(), Approval::Automatic, version, now);
    let id = store.save_proposal(AD, &draft, now).await.unwrap().unwrap();
    store
        .authorize_proposal(id, &draft.fingerprint(AD), now)
        .await
        .unwrap();
    let google = ChangesDuringValidation {
        store: &store,
        db: &f.ctx.db,
        google: Simulated::new(vec![live(80_000_000)]),
        change: DuringValidation::Manual,
        now,
    };
    assert_eq!(
        execute::run(&live_settings(&f), &store, &google, id, day(&f), 0, now)
            .await
            .unwrap(),
        Ran::Refused(Refusal::PolicyChanged)
    );
    assert_eq!(google.google.sent(), 0);
}

struct StaleReadback {
    google: Simulated,
    readback: Live,
    stale: std::sync::atomic::AtomicBool,
}

#[async_trait::async_trait]
impl keysend::optimize::ads::Ads for StaleReadback {
    async fn campaign(&self, id: i64) -> Result<Option<Live>, keysend::optimize::ads::AdsError> {
        if self.google.sent() > 0 && self.stale.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(Some(self.readback.clone()));
        }
        self.google.campaign(id).await
    }
    async fn apply(
        &self,
        action: &Action,
        validate: bool,
    ) -> Result<keysend::optimize::ads::Applied, keysend::optimize::ads::AdsError> {
        self.google.apply(action, validate).await
    }
}

#[tokio::test]
#[serial]
async fn a_stale_readback_never_causes_a_resend_or_a_compensating_write() {
    for lost in [false, true] {
        let f = Fixture::monitoring().await;
        let signed = user(&f, &f.a).await;
        let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
        let now = now(&f);
        policy(&store, Mode::Recommend, now).await;
        let id = approved(&store, cut(), now).await;
        let google = StaleReadback {
            google: Simulated::new(vec![live(80_000_000)]),
            readback: live(80_000_000),
            stale: true.into(),
        };
        if lost {
            google.google.fault(Fault::LostAnswer);
        }
        assert_eq!(
            execute::run(&live_settings(&f), &store, &google, id, day(&f), 0, now)
                .await
                .unwrap(),
            Ran::Uncertain
        );
        assert_eq!(
            execute::reconcile(&store, &google, id, now + 700)
                .await
                .unwrap(),
            Ran::Uncertain
        );
        assert_eq!(
            google.google.live(CAMPAIGN).unwrap().budget_micros,
            Some(64_000_000)
        );
        assert_eq!(google.google.sent(), 1);
        google
            .stale
            .store(false, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            execute::reconcile(&store, &google, id, now + 800)
                .await
                .unwrap(),
            Ran::Applied
        );
        assert_eq!(google.google.sent(), 1);
    }
}

#[tokio::test]
#[serial]
async fn the_intended_amount_on_a_different_budget_does_not_verify_a_change() {
    let f = Fixture::monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = now(&f);
    policy(&store, Mode::Recommend, now).await;
    let id = approved(&store, cut(), now).await;
    let google = StaleReadback {
        google: Simulated::new(vec![live(80_000_000)]),
        readback: Live {
            budget_id: Some(9002),
            ..live(64_000_000)
        },
        stale: true.into(),
    };
    assert_eq!(
        execute::run(&live_settings(&f), &store, &google, id, day(&f), 0, now)
            .await
            .unwrap(),
        Ran::Conflict
    );
    assert!(!store.execution(id).await.unwrap().unwrap().verified);
    assert_eq!(google.google.sent(), 1);
    assert_eq!(
        execute::run(&live_settings(&f), &store, &google, id, day(&f), 0, now)
            .await
            .unwrap(),
        Ran::Waiting
    );
    assert_eq!(google.google.sent(), 1);
}

#[tokio::test]
#[serial]
async fn a_daily_limit_tightened_during_validation_counts_other_executions() {
    let f = Fixture::monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = now(&f);
    policy(&store, Mode::Recommend, now).await;
    let first = approved(&store, Action::PauseCampaign { campaign_id: 702 }, now).await;
    let google = Simulated::new(vec![Live {
        campaign_id: 702,
        ..live(80_000_000)
    }]);
    assert_eq!(
        execute::run(&live_settings(&f), &store, &google, first, day(&f), 0, now)
            .await
            .unwrap(),
        Ran::Applied
    );
    let id = approved(&store, cut(), now).await;
    let google = ChangesDuringValidation {
        store: &store,
        db: &f.ctx.db,
        google: Simulated::new(vec![live(80_000_000)]),
        change: DuringValidation::DailyLimit,
        now,
    };
    assert_eq!(
        execute::run(&live_settings(&f), &store, &google, id, day(&f), 0, now)
            .await
            .unwrap(),
        Ran::Refused(Refusal::DailyChanges)
    );
    assert_eq!(google.google.sent(), 0);
}

#[tokio::test]
#[serial]
async fn entering_shadow_expires_an_existing_approval_before_google_is_read() {
    let f = Fixture::monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = now(&f);
    policy(&store, Mode::Recommend, now).await;
    let id = approved(&store, cut(), now).await;
    policy(&store, Mode::Shadow, now).await;
    let google = Simulated::new(vec![live(80_000_000)]);
    google.fault(Fault::Revoke);
    assert_eq!(
        execute::run(&live_settings(&f), &store, &google, id, day(&f), 0, now)
            .await
            .unwrap(),
        Ran::Waiting
    );
    assert_eq!(state(&store, id).await, ProposalState::Expired);
    assert!(google.requests().is_empty());
}

#[tokio::test]
#[serial]
async fn the_latest_read_only_mode_refuses_direct_approval_and_authorization() {
    for mode in [Mode::Shadow, Mode::Investigate] {
        let f = Fixture::monitoring().await;
        let signed = user(&f, &f.a).await;
        let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
        let now = now(&f);
        policy(&store, Mode::Autopilot, now).await;
        let version = policy(&store, mode, now).await;
        let draft = draft(cut(), Approval::Automatic, version, now);
        let id = store.save_proposal(AD, &draft, now).await.unwrap().unwrap();
        assert!(!store
            .authorize_proposal(id, &draft.fingerprint(AD), now)
            .await
            .unwrap());
        assert!(!store
            .approve_proposal(id, &draft.fingerprint(AD), now)
            .await
            .unwrap());
        assert_eq!(state(&store, id).await, ProposalState::Open);
    }
}

#[tokio::test]
#[serial]
async fn entering_a_read_only_mode_retires_pending_proposals_and_preserves_running_changes() {
    for (mode, running_state) in [
        (Mode::Shadow, ProposalState::Applying),
        (Mode::Investigate, ProposalState::Uncertain),
    ] {
        let f = Fixture::monitoring().await;
        let signed = user(&f, &f.a).await;
        let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
        let now = now(&f);
        let version = policy(&store, Mode::Recommend, now).await;
        let open = store
            .save_proposal(AD, &draft(cut(), Approval::Required, version, now), now)
            .await
            .unwrap()
            .unwrap();
        let approved = approved(&store, Action::PauseCampaign { campaign_id: 702 }, now).await;
        let running = self::approved(&store, Action::PauseCampaign { campaign_id: 703 }, now).await;
        let proposal = store.proposal(running).await.unwrap().unwrap();
        assert_eq!(
            store
                .claim_execution(&proposal, day(&f), false, 0, 3, 0, now)
                .await
                .unwrap(),
            keysend::workspace::optimization::Claim::Claimed
        );
        if running_state == ProposalState::Uncertain {
            store
                .finish_execution(
                    running,
                    &keysend::workspace::optimization::Finish {
                        state: running_state,
                        before: None,
                        after: None,
                        request_id: None,
                        error: Some("no_answer"),
                        refusal: None,
                        verified: false,
                        suspend: None,
                    },
                    now,
                )
                .await
                .unwrap();
        }
        policy(&store, mode, now).await;
        for (id, expected) in [
            (open, ProposalState::Superseded),
            (approved, ProposalState::Expired),
        ] {
            assert_eq!(state(&store, id).await, expected);
            assert_eq!(
                store
                    .optimization_events(Some(AD), Some(id), 1)
                    .await
                    .unwrap()[0]
                    .kind,
                expected.name()
            );
        }
        assert_eq!(state(&store, running).await, running_state);
        policy(&store, Mode::Recommend, now).await;
        for id in [open, approved] {
            let found = store.proposal(id).await.unwrap().unwrap();
            assert!(!store
                .approve_proposal(id, &found.fingerprint, now)
                .await
                .unwrap());
        }
    }
}

#[tokio::test]
#[serial]
async fn a_failed_retirement_audit_keeps_the_policy_and_proposals_unchanged() {
    let f = Fixture::monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = now(&f);
    let version = policy(&store, Mode::Recommend, now).await;
    let open = store
        .save_proposal(AD, &draft(cut(), Approval::Required, version, now), now)
        .await
        .unwrap()
        .unwrap();
    let approved = approved(&store, Action::PauseCampaign { campaign_id: 702 }, now).await;
    f.ctx.db.execute_unprepared("CREATE TRIGGER fail_retirement_audit BEFORE INSERT ON optimization_events WHEN NEW.kind='expired' BEGIN SELECT RAISE(ABORT,'injected retirement failure'); END").await.unwrap();
    let mut current = store.optimization_policy(AD).await.unwrap();
    current.policy.mode = Mode::Shadow;
    assert!(store
        .save_optimization_policy(AD, &current.policy, Standing::Active, "", now)
        .await
        .unwrap_err()
        .to_string()
        .contains("injected retirement failure"));
    assert_eq!(
        store.optimization_policy(AD).await.unwrap().version,
        version
    );
    assert_eq!(state(&store, open).await, ProposalState::Open);
    assert_eq!(state(&store, approved).await, ProposalState::Approved);
    assert_eq!(
        store
            .optimization_events(Some(AD), Some(open), 1)
            .await
            .unwrap()[0]
            .kind,
        "proposed"
    );
}

/// An Ad Account in Shadow mode with one evaluation: a proposed budget cut and a held Signal.
async fn shadowed(
    f: &Fixture,
    store: &WorkspaceStore<'_>,
) -> (i64, keysend::optimize::shadow::Evaluation) {
    use keysend::optimize::{
        propose::{Decision, Hold},
        shadow::Evaluation,
    };
    let now = now(f);
    let version = policy(store, Mode::Shadow, now).await;
    let shown = draft(cut(), Approval::Required, version, now);
    let evaluation = Evaluation {
        policy: Policy {
            mode: Mode::Shadow,
            ..Default::default()
        },
        policy_version: version,
        data_day: 29,
        day: 30,
        at: now,
        decisions: vec![
            Decision::Propose(Box::new(shown.clone())),
            Decision::Hold(Box::new(shown.signal.clone()), Hold::Cooldown),
        ],
        abstention: None,
        snapshot: serde_json::json!({"days": [{"day": 1}], "campaigns": [1], "changes": [], "findings": [], "tracking_reliable": true}),
    };
    store.save_shadow(AD, &evaluation).await.unwrap();
    (store.shadows(None, 1).await.unwrap()[0].id, evaluation)
}

#[tokio::test]
#[serial]
async fn a_shadow_evaluation_shows_what_keysend_would_propose_and_offers_nothing_to_approve() {
    let f = Fixture::monitoring().await;
    let owner = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &owner.workspace);
    let (id, _) = shadowed(&f, &store).await;
    store
        .record_shadow_event(
            id,
            "observation",
            &serde_json::json!({"observations": [{"campaign_id": CAMPAIGN, "data_day": 33, "settled_days": 2, "before_days": 5, "before_spend": 10.0, "before_conversions": 1.0, "after_spend": 4.0, "after_conversions": 0.0, "tracking_reliable": false, "external_changes": 2}]}),
            now(&f),
        )
        .await
        .unwrap();
    let index = f.request.get("/optimize").add_cookie(f.a.clone()).await;
    assert_eq!(index.status_code(), 200);
    assert!(index.text().contains(&format!("/optimize/shadows/{id}")));
    let page = f
        .request
        .get(&format!("/optimize/shadows/{id}"))
        .add_cookie(f.a.clone())
        .await;
    assert_eq!(page.status_code(), 200);
    let page = page.text();
    assert!(page.contains("Would propose"));
    assert!(page.contains("No change for Search - Real"));
    assert!(page.contains("Days with data: 5 of 14"));
    assert!(page.contains("Conversion tracking was not reliable in this period"));
    assert!(page.contains("They do not show what the change would have done"));
    assert!(!page.contains("/approve"));
    let other = f
        .request
        .get(&format!("/optimize/shadows/{id}"))
        .add_cookie(f.b.clone())
        .await;
    assert_eq!(other.status_code(), 404);
}

#[tokio::test]
#[serial]
async fn the_user_rates_a_shadow_decision_and_a_wrong_decision_or_a_long_note_is_refused() {
    let f = Fixture::monitoring().await;
    let owner = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &owner.workspace);
    let (id, _) = shadowed(&f, &store).await;
    let send = |form: Vec<(&'static str, String)>| {
        f.request
            .post(&format!("/optimize/shadows/{id}/feedback"))
            .add_cookie(f.a.clone())
            .add_header("origin", ORIGIN)
            .form(&form)
    };
    let sent = send(vec![
        ("item", "1".into()),
        ("feedback", "too_risky".into()),
        ("note", "Busy season".into()),
    ])
    .await;
    assert_eq!(sent.status_code(), 303);
    for wrong in [
        vec![("item", "2".into()), ("feedback", "useful".into())],
        vec![("item", "0".into()), ("feedback", "nope".into())],
        vec![
            ("item", "0".into()),
            ("feedback", "useful".into()),
            ("note", "x".repeat(1001)),
        ],
    ] {
        assert_eq!(send(wrong).await.status_code(), 422);
    }
    let page = f
        .request
        .get(&format!("/optimize/shadows/{id}"))
        .add_cookie(f.a.clone())
        .await
        .text();
    assert!(page.contains("Your feedback: Too risky"));
    assert!(page.contains("Busy season"));
    let theirs = f
        .request
        .post(&format!("/optimize/shadows/{id}/feedback"))
        .add_cookie(f.b.clone())
        .add_header("origin", ORIGIN)
        .form(&[("item", "0"), ("feedback", "useful")])
        .await;
    assert_eq!(theirs.status_code(), 404);
}

#[tokio::test]
#[serial]
async fn a_shadow_evaluation_without_a_target_says_why_it_proposes_nothing() {
    let f = Fixture::monitoring().await;
    let owner = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &owner.workspace);
    let (_, evaluation) = shadowed(&f, &store).await;
    let empty = keysend::optimize::shadow::Evaluation {
        decisions: vec![],
        abstention: Some(keysend::optimize::shadow::Abstention::MissingCommercialTarget),
        data_day: 30,
        ..evaluation
    };
    store.save_shadow(AD, &empty).await.unwrap();
    let id = store.shadows(None, 1).await.unwrap()[0].id;
    let page = f
        .request
        .get(&format!("/optimize/shadows/{id}"))
        .add_cookie(f.a.clone())
        .await;
    assert_eq!(page.status_code(), 200);
    assert!(page.text().contains("no target cost per conversion"));
}

#[tokio::test]
#[serial]
async fn the_user_gives_a_reason_when_rejecting_a_proposal() {
    let f = Fixture::monitoring().await;
    let owner = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &owner.workspace);
    let now = now(&f);
    let version = policy(&store, Mode::Recommend, now).await;
    let mut ids = Vec::new();
    for reason in ["Busy season", ""] {
        let action = Action::AdjustCampaignBudget {
            campaign_id: CAMPAIGN + ids.len() as i64,
            budget_id: BUDGET,
            from_micros: 80_000_000,
            to_micros: 60_000_000,
        };
        let id = store
            .save_proposal(AD, &draft(action, Approval::Required, version, now), now)
            .await
            .unwrap()
            .unwrap();
        let response = f
            .request
            .post(&format!("/optimize/proposals/{id}/reject"))
            .add_cookie(f.a.clone())
            .add_header("origin", ORIGIN)
            .form(&[("reason", reason)])
            .await;
        assert_eq!(response.status_code(), 303);
        ids.push(id);
    }
    let page = |id: i64| {
        f.request
            .get(&format!("/optimize/proposals/{id}"))
            .add_cookie(f.a.clone())
    };
    assert!(page(ids[0])
        .await
        .text()
        .contains("You rejected this change. Reason: Busy season"));
    assert!(page(ids[1])
        .await
        .text()
        .contains("You rejected this change. No reason given."));
}

#[tokio::test]
#[serial]
async fn the_user_cannot_select_a_write_mode_above_the_installation_ceiling() {
    let f = Fixture::configured_source(false, false, true, "optimization-shadow-ceiling").await;
    f.prepare_monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let response = f
        .request
        .post(&format!("/ad-accounts/{AD}/optimize"))
        .add_cookie(f.a.clone())
        .add_header("origin", ORIGIN)
        .form(&[
            ("mode", "recommend"),
            ("max_decrease_percent", "20"),
            ("max_increase_percent", "10"),
            ("envelope", ""),
            ("max_daily_increase", "0"),
            ("changes_per_day", "3"),
            ("cooldown_days", "7"),
            ("min_conversions", "10"),
            ("target_cost", ""),
        ])
        .await;
    assert_eq!(response.status_code(), 422);
    assert_eq!(
        store.optimization_policy(AD).await.unwrap().policy.mode,
        Mode::Investigate
    );
    assert!(f.calls.lock().unwrap().google.mutations.is_empty());
}

#[tokio::test]
#[serial]
async fn a_worker_that_lost_its_reservation_cannot_fail_the_change_or_suspend_the_account() {
    let f = Fixture::monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = now(&f);
    policy(&store, Mode::Recommend, now).await;
    let id = approved(&store, cut(), now).await;
    let google = ChangesDuringValidation {
        store: &store,
        db: &f.ctx.db,
        google: Simulated::new(vec![live(80_000_000)]),
        change: DuringValidation::ReconcileThenAccess(id),
        now,
    };
    assert_eq!(
        execute::run(&live_settings(&f), &store, &google, id, day(&f), 0, now)
            .await
            .unwrap(),
        Ran::Waiting
    );
    assert_eq!(state(&store, id).await, ProposalState::Uncertain);
    assert_eq!(
        store.optimization_policy(AD).await.unwrap().standing,
        Standing::Active
    );
    assert_eq!(google.google.sent(), 0);
}

#[tokio::test]
#[serial]
async fn a_dispatch_audit_failure_prevents_sending_the_change() {
    let f = Fixture::monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = now(&f);
    policy(&store, Mode::Recommend, now).await;
    let id = approved(&store, cut(), now).await;
    f.ctx.db.execute_unprepared("CREATE TRIGGER fail_dispatch BEFORE INSERT ON optimization_events WHEN NEW.kind='dispatching' BEGIN SELECT RAISE(ABORT,'injected dispatch audit failure'); END").await.unwrap();
    let google = Simulated::new(vec![live(80_000_000)]);
    assert!(
        execute::run(&live_settings(&f), &store, &google, id, day(&f), 0, now)
            .await
            .is_err()
    );
    assert_eq!(
        google
            .requests()
            .iter()
            .map(|(_, validate)| *validate)
            .collect::<Vec<_>>(),
        [true]
    );
    assert_eq!(google.sent(), 0);
    assert_eq!(
        store.decision_quality().await.unwrap().execution_attempts,
        0
    );
    assert_eq!(state(&store, id).await, ProposalState::Applying);
}

#[tokio::test]
#[serial]
async fn a_campaign_protected_during_validation_counts_as_one_policy_rejection_and_is_never_sent() {
    let f = Fixture::monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = now(&f);
    policy(&store, Mode::Recommend, now).await;
    let id = approved(&store, cut(), now).await;
    let google = ChangesDuringValidation {
        store: &store,
        db: &f.ctx.db,
        google: Simulated::new(vec![live(80_000_000)]),
        change: DuringValidation::Protect,
        now,
    };
    assert_eq!(
        execute::run(&live_settings(&f), &store, &google, id, day(&f), 0, now)
            .await
            .unwrap(),
        Ran::Refused(Refusal::NotEligible)
    );
    let quality = store.decision_quality().await.unwrap();
    assert_eq!(quality.policy_rejections, 1);
    assert_eq!(quality.execution_attempts, 0);
    assert_eq!(google.google.sent(), 0);
    assert!(google
        .google
        .requests()
        .iter()
        .all(|(_, validate)| *validate));
    assert!(f.calls.lock().unwrap().google.mutations.is_empty());
    let audit = &store
        .optimization_events(Some(AD), Some(id), 1)
        .await
        .unwrap()[0];
    assert_eq!(audit.kind, "failed");
    assert_eq!(audit.detail["refusal"], Refusal::NotEligible.name());
    assert!(audit.detail["error"].is_null());
}
