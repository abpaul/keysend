use super::*;
use keysend::{
    optimize::{
        ads::{Ads, AdsError, Applied, Fault, Simulated},
        execute::{self, Ran},
        policy::{Live, Policy, Standing},
        signals::{Kind, Severity, Signal},
        Action, Approval, Draft, Mode, ProposalState,
    },
    settings::Settings,
    workspace::optimization::{Claim, Recovery, Report, NOT_APPLIED},
};
use sea_orm::ConnectionTrait;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Notify;

const AD: &str = "4829153307";
const CAMPAIGN: i64 = 701;
const BUDGET: i64 = 9001;
const NOTE: &str = "Google Ads support, case 4-2210: the budget change was never applied.";

async fn user(f: &Fixture, cookie: &Cookie<'static>) -> identity::SignedIn {
    identity::authenticate(&f.ctx.db, cookie.value())
        .await
        .unwrap()
        .unwrap()
}

fn settings(f: &Fixture) -> Settings {
    let mut settings = Settings::load(&f.ctx).unwrap();
    settings.optimize.live = true;
    settings.optimize.max_mode = Mode::Autopilot;
    settings
}

fn now(f: &Fixture) -> i64 {
    Settings::load(&f.ctx).unwrap().now().timestamp()
}

fn day(f: &Fixture) -> i64 {
    keysend::workspace::history::day_number(Settings::load(&f.ctx).unwrap().now().date_naive())
}

fn live() -> Live {
    Live {
        campaign_id: CAMPAIGN,
        status: "ENABLED".into(),
        budget_id: Some(BUDGET),
        budget_micros: Some(80_000_000),
        budget_shared: false,
        budget_campaigns: 1,
        bidding_strategy: "MAXIMIZE_CONVERSIONS".into(),
    }
}

fn draft(action: Action, version: i64, now: i64) -> Draft {
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
        approval: Approval::Required,
        policy_version: version,
        reasons: vec![],
        created_at: now,
        expires_at: now + 86_400,
    }
}

async fn recommend(store: &WorkspaceStore<'_>, now: i64) -> i64 {
    let policy = Policy {
        mode: Mode::Recommend,
        envelope_micros: Some(1_000_000_000),
        max_daily_increase_micros: 50_000_000,
        ..Default::default()
    };
    store
        .save_optimization_policy(AD, &policy, Standing::Active, "", now)
        .await
        .unwrap()
}

async fn approved(store: &WorkspaceStore<'_>, action: Action, now: i64) -> i64 {
    let version = store.optimization_policy(AD).await.unwrap().version;
    let draft = draft(action, version, now);
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

/// A budget cut that Google Ads never confirmed and that no read could settle, with an open
/// and an approved Proposal of the same Ad Account behind it.
struct Stuck {
    google: Simulated,
    uncertain: i64,
    fingerprint: String,
    open: i64,
    approved: i64,
}

async fn stuck(f: &Fixture, store: &WorkspaceStore<'_>) -> Stuck {
    let now = now(f);
    let version = recommend(store, now).await;
    let uncertain = approved(
        store,
        Action::AdjustCampaignBudget {
            campaign_id: CAMPAIGN,
            budget_id: BUDGET,
            from_micros: 80_000_000,
            to_micros: 64_000_000,
        },
        now,
    )
    .await;
    let google = Simulated::new(vec![live()]);
    google.fault(Fault::Timeout);
    google.fault(Fault::ReadUnknown);
    assert_eq!(
        execute::run(&settings(f), store, &google, uncertain, day(f), 0, now)
            .await
            .unwrap(),
        Ran::Uncertain
    );
    let open = store
        .save_proposal(
            AD,
            &draft(Action::PauseCampaign { campaign_id: 703 }, version, now),
            now,
        )
        .await
        .unwrap()
        .unwrap();
    let approved = approved(store, Action::PauseCampaign { campaign_id: 702 }, now).await;
    Stuck {
        google,
        fingerprint: store
            .proposal(uncertain)
            .await
            .unwrap()
            .unwrap()
            .fingerprint,
        uncertain,
        open,
        approved,
    }
}

/// Google Ads that counts the reads of a Campaign and, when it holds, answers a read only
/// after the test releases it. It never applies a change.
struct Held {
    answer: Result<Option<Live>, AdsError>,
    hold: bool,
    reads: AtomicUsize,
    reading: Notify,
    release: Notify,
}

impl Held {
    fn new(answer: Result<Option<Live>, AdsError>, hold: bool) -> Self {
        Self {
            answer,
            hold,
            reads: AtomicUsize::new(0),
            reading: Notify::new(),
            release: Notify::new(),
        }
    }
}

#[async_trait::async_trait]
impl Ads for Held {
    async fn campaign(&self, _: i64) -> Result<Option<Live>, AdsError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if self.hold {
            self.reading.notify_one();
            self.release.notified().await;
        }
        self.answer.clone()
    }

    async fn apply(&self, _: &Action, _: bool) -> Result<Applied, AdsError> {
        panic!("a reconciliation never sends a change")
    }
}

async fn report(
    f: &Fixture,
    cookie: &Cookie<'static>,
    id: i64,
    form: &[(&str, &str)],
) -> axum_test::TestResponse {
    f.request
        .post(&format!("/optimize/proposals/{id}/recover"))
        .add_cookie(cookie.clone())
        .add_header("origin", ORIGIN)
        .form(&form)
        .await
}

#[tokio::test]
#[serial]
async fn the_user_records_that_google_ads_confirmed_an_uncertain_change_was_not_applied() {
    let f = Fixture::monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let stuck = stuck(&f, &store).await;
    let sent = stuck.google.requests().len();
    let page = f
        .request
        .get(&format!("/optimize/proposals/{}", stuck.uncertain))
        .add_cookie(f.a.clone())
        .await
        .text();
    assert!(page.contains("Report confirmation from Google Ads"));
    assert!(page.contains(&format!("/optimize/proposals/{}/recover", stuck.uncertain)));

    let response = report(
        &f,
        &f.a,
        stuck.uncertain,
        &[
            ("fingerprint", stuck.fingerprint.as_str()),
            ("note", NOTE),
            ("confirmed", "yes"),
        ],
    )
    .await;
    assert_eq!(response.status_code(), 303);
    assert_eq!(
        response.header("location"),
        format!("/optimize/proposals/{}", stuck.uncertain)
    );

    assert_eq!(state(&store, stuck.uncertain).await, ProposalState::Failed);
    assert_eq!(state(&store, stuck.open).await, ProposalState::Superseded);
    assert_eq!(state(&store, stuck.approved).await, ProposalState::Expired);
    let current = store.optimization_policy(AD).await.unwrap();
    assert_eq!(current.policy.mode, Mode::Investigate);
    assert_eq!(current.standing, Standing::Suspended);
    let execution = store.execution(stuck.uncertain).await.unwrap().unwrap();
    assert_eq!(execution.error.as_deref(), Some(NOT_APPLIED));
    assert!(!execution.verified);
    // The audit keeps who reported it; no page shows the User's ID.
    let audit = &store
        .optimization_events(Some(AD), Some(stuck.uncertain), 1)
        .await
        .unwrap()[0];
    assert_eq!(audit.kind, "failed");
    assert_eq!(audit.detail["user_id"], signed.user.id.as_str());
    assert_eq!(audit.detail["note"], NOTE);
    assert_eq!(audit.detail["verified"], false);

    let page = f
        .request
        .get(&format!("/optimize/proposals/{}", stuck.uncertain))
        .add_cookie(f.a.clone())
        .await
        .text();
    assert!(page.contains("KeySend did not verify this"));
    assert!(page.contains(NOTE));
    assert!(!page.contains(&format!("/optimize/proposals/{}/recover", stuck.uncertain)));
    // The policy change belongs to the Ad Account, so the activity of the Workspace shows it.
    let activity = f
        .request
        .get("/optimize/activity")
        .add_cookie(f.a.clone())
        .await
        .text();
    assert!(activity.contains("Optimize switched to Investigate and is suspended"));

    // The change is never sent again, read again or undone.
    let reads = Held::new(Ok(Some(live())), false);
    assert_eq!(
        execute::reconcile(&store, &reads, stuck.uncertain, now(&f) + 700)
            .await
            .unwrap(),
        Ran::Waiting
    );
    assert_eq!(reads.reads.load(Ordering::SeqCst), 0);
    assert_eq!(stuck.google.requests().len(), sent);
    assert_eq!(stuck.google.sent(), 1);
    assert_eq!(
        stuck.google.live(CAMPAIGN).unwrap().budget_micros,
        Some(80_000_000)
    );
    assert!(f.calls.lock().unwrap().google.mutations.is_empty());
}

#[tokio::test]
#[serial]
async fn the_same_report_again_changes_nothing() {
    let f = Fixture::monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let stuck = stuck(&f, &store).await;
    let form = [
        ("fingerprint", stuck.fingerprint.as_str()),
        ("note", NOTE),
        ("confirmed", "yes"),
    ];
    assert_eq!(
        report(&f, &f.a, stuck.uncertain, &form).await.status_code(),
        303
    );
    let version = store.optimization_policy(AD).await.unwrap().version;
    let events = store
        .optimization_events(Some(AD), None, 500)
        .await
        .unwrap()
        .len();
    assert_eq!(
        report(&f, &f.a, stuck.uncertain, &form).await.status_code(),
        303
    );
    assert_eq!(
        store.optimization_policy(AD).await.unwrap().version,
        version
    );
    assert_eq!(
        store
            .optimization_events(Some(AD), None, 500)
            .await
            .unwrap()
            .len(),
        events
    );
    assert_eq!(state(&store, stuck.uncertain).await, ProposalState::Failed);
}

#[tokio::test]
#[serial]
async fn another_workspace_cannot_record_a_report() {
    let f = Fixture::monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let stuck = stuck(&f, &store).await;
    let response = report(
        &f,
        &f.b,
        stuck.uncertain,
        &[
            ("fingerprint", stuck.fingerprint.as_str()),
            ("note", NOTE),
            ("confirmed", "yes"),
        ],
    )
    .await;
    assert_eq!(response.status_code(), 404);
    let other = user(&f, &f.b).await;
    assert_eq!(
        WorkspaceStore::new(&f.ctx.db, &other.workspace)
            .recover_uncertain(
                stuck.uncertain,
                &stuck.fingerprint,
                &Report {
                    note: NOTE,
                    user_id: &other.user.id
                },
                now(&f)
            )
            .await
            .unwrap(),
        Recovery::NotUncertain
    );
    assert_eq!(
        state(&store, stuck.uncertain).await,
        ProposalState::Uncertain
    );
    assert_eq!(
        store.optimization_policy(AD).await.unwrap().policy.mode,
        Mode::Recommend
    );
}

#[tokio::test]
#[serial]
async fn a_report_without_a_note_or_the_confirmation_is_refused() {
    let f = Fixture::monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let stuck = stuck(&f, &store).await;
    let long = "x".repeat(1001);
    for form in [
        vec![("fingerprint", stuck.fingerprint.as_str()), ("note", NOTE)],
        vec![
            ("fingerprint", stuck.fingerprint.as_str()),
            ("note", "   "),
            ("confirmed", "yes"),
        ],
        vec![
            ("fingerprint", stuck.fingerprint.as_str()),
            ("note", long.as_str()),
            ("confirmed", "yes"),
        ],
    ] {
        let response = report(&f, &f.a, stuck.uncertain, &form).await;
        assert_eq!(response.status_code(), 422);
        assert!(response
            .text()
            .contains("Describe what Google Ads confirmed, in up to 1000 characters"));
    }
    assert_eq!(
        state(&store, stuck.uncertain).await,
        ProposalState::Uncertain
    );
    assert_eq!(state(&store, stuck.open).await, ProposalState::Open);
    assert_eq!(
        store.optimization_policy(AD).await.unwrap().policy.mode,
        Mode::Recommend
    );
}

#[tokio::test]
#[serial]
async fn only_an_uncertain_change_with_its_own_fingerprint_can_be_recorded() {
    let f = Fixture::monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let stuck = stuck(&f, &store).await;
    let wrong = report(
        &f,
        &f.a,
        stuck.uncertain,
        &[
            ("fingerprint", "other"),
            ("note", NOTE),
            ("confirmed", "yes"),
        ],
    )
    .await;
    assert_eq!(wrong.status_code(), 409);
    assert_eq!(
        state(&store, stuck.uncertain).await,
        ProposalState::Uncertain
    );

    // A change that a worker is still sending cannot be recorded as not applied.
    let f = Fixture::monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = now(&f);
    recommend(&store, now).await;
    let id = approved(
        &store,
        Action::PauseCampaign {
            campaign_id: CAMPAIGN,
        },
        now,
    )
    .await;
    let proposal = store.proposal(id).await.unwrap().unwrap();
    assert_eq!(
        store
            .claim_execution(&proposal, day(&f), false, 0, 3, 0, now)
            .await
            .unwrap(),
        Claim::Claimed
    );
    let applying = report(
        &f,
        &f.a,
        id,
        &[
            ("fingerprint", proposal.fingerprint.as_str()),
            ("note", NOTE),
            ("confirmed", "yes"),
        ],
    )
    .await;
    assert_eq!(applying.status_code(), 409);
    assert_eq!(state(&store, id).await, ProposalState::Applying);
    assert!(!f
        .request
        .get(&format!("/optimize/proposals/{id}"))
        .add_cookie(f.a.clone())
        .await
        .text()
        .contains(&format!("/optimize/proposals/{id}/recover")));
}

#[tokio::test]
#[serial]
async fn a_failed_audit_keeps_the_change_uncertain_and_the_policy_unchanged() {
    let f = Fixture::monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let stuck = stuck(&f, &store).await;
    let version = store.optimization_policy(AD).await.unwrap().version;
    f.ctx.db.execute_unprepared("CREATE TRIGGER fail_recovery_audit BEFORE INSERT ON optimization_events WHEN NEW.kind='policy' BEGIN SELECT RAISE(ABORT,'injected recovery failure'); END").await.unwrap();
    assert!(store
        .recover_uncertain(
            stuck.uncertain,
            &stuck.fingerprint,
            &Report {
                note: NOTE,
                user_id: &signed.user.id
            },
            now(&f)
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("injected recovery failure"));
    assert_eq!(
        state(&store, stuck.uncertain).await,
        ProposalState::Uncertain
    );
    assert_eq!(state(&store, stuck.open).await, ProposalState::Open);
    assert_eq!(state(&store, stuck.approved).await, ProposalState::Approved);
    assert_eq!(
        store.optimization_policy(AD).await.unwrap().version,
        version
    );
    let execution = store.execution(stuck.uncertain).await.unwrap().unwrap();
    assert_ne!(execution.error.as_deref(), Some(NOT_APPLIED));
    assert!(store
        .optimization_events(Some(AD), Some(stuck.uncertain), 50)
        .await
        .unwrap()
        .iter()
        .all(|event| event.detail["recovery"].is_null()));
}

#[tokio::test]
#[serial]
async fn changes_run_again_only_after_the_user_saves_the_settings() {
    let f = Fixture::monitoring().await;
    let signed = user(&f, &f.a).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let stuck = stuck(&f, &store).await;
    assert_eq!(
        report(
            &f,
            &f.a,
            stuck.uncertain,
            &[
                ("fingerprint", stuck.fingerprint.as_str()),
                ("note", NOTE),
                ("confirmed", "yes"),
            ],
        )
        .await
        .status_code(),
        303
    );
    let now = now(&f);
    let version = store.optimization_policy(AD).await.unwrap().version;
    let next = draft(Action::PauseCampaign { campaign_id: 704 }, version, now);
    let id = store.save_proposal(AD, &next, now).await.unwrap().unwrap();
    assert!(!store
        .approve_proposal(id, &next.fingerprint(AD), now)
        .await
        .unwrap());
    // Resuming the Autopilot is not a new choice of mode: the Ad Account stays in Investigate.
    let resumed = f
        .request
        .post(&format!("/ad-accounts/{AD}/optimize/autopilot"))
        .add_cookie(f.a.clone())
        .add_header("origin", ORIGIN)
        .form(&[("stop", "resume")])
        .await;
    assert_eq!(resumed.status_code(), 303);
    assert_eq!(
        store.optimization_policy(AD).await.unwrap().policy.mode,
        Mode::Investigate
    );
    assert!(!store
        .approve_proposal(id, &next.fingerprint(AD), now)
        .await
        .unwrap());
    // Saving the settings with a write mode lets a new Proposal be approved.
    let version = recommend(&store, now).await;
    let next = draft(Action::PauseCampaign { campaign_id: 705 }, version, now);
    let id = store.save_proposal(AD, &next, now).await.unwrap().unwrap();
    assert!(store
        .approve_proposal(id, &next.fingerprint(AD), now)
        .await
        .unwrap());
}

#[tokio::test]
#[serial]
async fn a_late_reconciliation_read_cannot_suspend_the_policy_the_user_resumed_after_a_report() {
    let changed = Live {
        budget_micros: Some(72_000_000),
        ..live()
    };
    for answer in [Ok(Some(changed)), Err(AdsError::Access)] {
        let f = Fixture::monitoring().await;
        let signed = user(&f, &f.a).await;
        let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
        let stuck = stuck(&f, &store).await;
        let now = now(&f);
        let google = Held::new(answer, true);
        let resumed = Policy {
            mode: Mode::Autopilot,
            envelope_micros: Some(1_000_000_000),
            max_daily_increase_micros: 50_000_000,
            automatic: keysend::optimize::policy::AUTOMATIC_KINDS
                .into_iter()
                .collect(),
            ..Default::default()
        };
        let (ran, version) = tokio::join!(
            execute::reconcile(&store, &google, stuck.uncertain, now + 700),
            async {
                google.reading.notified().await;
                assert_eq!(
                    store
                        .recover_uncertain(
                            stuck.uncertain,
                            &stuck.fingerprint,
                            &Report {
                                note: NOTE,
                                user_id: &signed.user.id
                            },
                            now + 701,
                        )
                        .await
                        .unwrap(),
                    Recovery::Recovered
                );
                let version = store
                    .save_optimization_policy(AD, &resumed, Standing::Active, "", now + 702)
                    .await
                    .unwrap();
                google.release.notify_one();
                version
            }
        );
        assert_eq!(ran.unwrap(), Ran::Waiting);
        assert_eq!(google.reads.load(Ordering::SeqCst), 1);
        assert_eq!(state(&store, stuck.uncertain).await, ProposalState::Failed);
        let execution = store.execution(stuck.uncertain).await.unwrap().unwrap();
        assert_eq!(execution.error.as_deref(), Some(NOT_APPLIED));
        let audit = &store
            .optimization_events(Some(AD), Some(stuck.uncertain), 1)
            .await
            .unwrap()[0];
        assert_eq!(audit.kind, "failed");
        assert_eq!(audit.detail["recovery"], NOT_APPLIED);
        let current = store.optimization_policy(AD).await.unwrap();
        assert_eq!(current.version, version);
        assert_eq!(current.policy.mode, Mode::Autopilot);
        assert_eq!(current.standing, Standing::Active);
    }
}
