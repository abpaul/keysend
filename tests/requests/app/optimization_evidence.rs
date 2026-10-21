use super::*;
use keysend::{
    monitoring::{audit::Counts, economics::Economics},
    optimize::{
        cycle,
        policy::{Policy, Standing},
        propose,
        signals::{Kind, Severity, Signal},
        Action, Approval, Draft, Mode, ProposalState,
    },
    settings::Settings,
    workspace::history::{date_of, day_number, CampaignDay},
};
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

const AD: &str = "4829153307";
const CAMPAIGN: i64 = 701;

fn live_settings(f: &Fixture) -> Settings {
    let mut settings = Settings::load(&f.ctx).unwrap();
    settings.optimize.live = true;
    settings.optimize.max_mode = Mode::Autopilot;
    settings
}

/// A cut of the budget of a Campaign whose cost per conversion was above the target.
fn cut(version: i64, now: i64) -> Draft {
    Draft {
        key: format!("adjust_campaign_budget:{CAMPAIGN}"),
        action: Action::AdjustCampaignBudget {
            campaign_id: CAMPAIGN,
            budget_id: 9001,
            from_micros: 80_000_000,
            to_micros: 64_000_000,
        },
        signal: Signal {
            kind: Kind::CostAboveTarget,
            campaign_id: Some(CAMPAIGN),
            campaign: Some("Search - Real".into()),
            term: None,
            from_day: 0,
            to_day: 29,
            spend: 300.0,
            clicks: 900.0,
            conversions: 30.0,
            target: Some(5.0),
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
        expires_at: now + propose::EXPIRY_SECONDS,
    }
}

/// An Ad Account in Recommend mode with a target of 5 a conversion, whose Campaign spent 10
/// a day for one conversion a day over the last 90 days: twice the target.
async fn costly(f: &Fixture, store: &WorkspaceStore<'_>, workspace: &str, now: i64) -> i64 {
    f.ctx
        .db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "UPDATE workspace_trials SET started_at=? WHERE workspace_id=?",
            vec![now.into(), workspace.into()],
        ))
        .await
        .unwrap();
    let today = day_number(Settings::load(&f.ctx).unwrap().now().date_naive());
    let days: Vec<_> = (today - 90..today)
        .map(|day| CampaignDay {
            day: date_of(day).unwrap(),
            campaign_id: CAMPAIGN,
            device: 1,
            counts: Counts {
                impressions: 200.0,
                clicks: 30.0,
                cost: 10.0,
                conversions: 1.0,
                value: 0.0,
            },
        })
        .collect();
    store
        .save_history(
            AD,
            date_of(today - 90).unwrap(),
            date_of(today - 1).unwrap(),
            &[],
            &days,
            &[],
        )
        .await
        .unwrap();
    store
        .save_optimization_policy(
            AD,
            &Policy {
                mode: Mode::Recommend,
                target_cost_micros: Some(5_000_000),
                evaluate_every_days: 1,
                ..Default::default()
            },
            Standing::Active,
            "user",
            now,
        )
        .await
        .unwrap()
}

/// The profile's economics: a conversion is worth 1,000, so 10 a conversion pays.
async fn profitable(store: &WorkspaceStore<'_>) {
    let mut profile = store.profile(AD).await.unwrap();
    profile.economics = Economics {
        customer_value: Some(1000.0),
        close_rate: Some(1.0),
        ..Default::default()
    };
    assert!(store.save_profile(AD, &profile).await.unwrap());
}

#[tokio::test]
#[serial]
async fn an_open_budget_cut_is_superseded_once_the_profiles_economics_show_the_campaign_paying() {
    let f = Fixture::monitoring().await;
    f.prepare_monitoring().await;
    let signed = identity::authenticate(&f.ctx.db, f.a.value())
        .await
        .unwrap()
        .unwrap();
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let settings = Settings::load(&f.ctx).unwrap();
    let now = settings.now().timestamp();
    let version = costly(&f, &store, signed.workspace.id(), now).await;
    let proposal = store
        .save_proposal(AD, &cut(version, now), now)
        .await
        .unwrap()
        .unwrap();
    cycle::evaluate(&settings, &store, AD, now).await.unwrap();
    assert_eq!(
        store.proposal(proposal).await.unwrap().unwrap().state,
        ProposalState::Open,
        "without economics the cost above the target still stands"
    );
    profitable(&store).await;
    cycle::evaluate(&settings, &store, AD, now + 86_400 + 1)
        .await
        .unwrap();
    assert_eq!(
        store.proposal(proposal).await.unwrap().unwrap().state,
        ProposalState::Superseded,
        "a cut of a Campaign that pays for itself stays open"
    );
}

#[tokio::test]
#[serial]
async fn an_approved_budget_cut_is_not_sent_once_the_profiles_economics_show_the_campaign_paying() {
    let f = Fixture::monitoring().await;
    f.prepare_monitoring().await;
    let signed = identity::authenticate(&f.ctx.db, f.a.value())
        .await
        .unwrap()
        .unwrap();
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let settings = live_settings(&f);
    let now = settings.now().timestamp();
    let version = costly(&f, &store, signed.workspace.id(), now).await;
    let draft = cut(version, now);
    let proposal = store.save_proposal(AD, &draft, now).await.unwrap().unwrap();
    assert!(store
        .approve_proposal(proposal, &draft.fingerprint(AD), now)
        .await
        .unwrap());
    profitable(&store).await;
    f.calls.lock().unwrap().google.live_budget = Some(80_000_000);
    let ran = cycle::execute_one(&settings, &store, proposal, now)
        .await
        .unwrap();
    assert!(
        matches!(ran, keysend::optimize::execute::Ran::Refused(_)),
        "{ran:?}"
    );
    {
        let calls = f.calls.lock().unwrap();
        assert_eq!(calls.google.live_budget, Some(80_000_000));
        assert!(
            calls.google.mutations.is_empty(),
            "nothing is validated or sent to Google Ads"
        );
    }
    assert_ne!(
        store.proposal(proposal).await.unwrap().unwrap().state,
        ProposalState::Applied
    );
}

#[tokio::test]
#[serial]
async fn an_applied_change_waits_for_collected_days_before_its_outcome_is_judged() {
    let f = Fixture::monitoring().await;
    f.prepare_monitoring().await;
    let signed = identity::authenticate(&f.ctx.db, f.a.value())
        .await
        .unwrap()
        .unwrap();
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let mut settings = live_settings(&f);
    let now = settings.now().timestamp();
    let day = day_number(settings.now().date_naive());
    costly(&f, &store, signed.workspace.id(), now).await;
    let mut policy = store.optimization_policy(AD).await.unwrap().policy;
    policy.observation_days = 7;
    let version = store
        .save_optimization_policy(AD, &policy, Standing::Active, "user", now)
        .await
        .unwrap();
    let draft = cut(version, now);
    let proposal = store.save_proposal(AD, &draft, now).await.unwrap().unwrap();
    assert!(store
        .approve_proposal(proposal, &draft.fingerprint(AD), now)
        .await
        .unwrap());
    let google = keysend::optimize::ads::Simulated::new(vec![keysend::optimize::policy::Live {
        campaign_id: CAMPAIGN,
        status: "ENABLED".into(),
        budget_id: Some(9001),
        budget_micros: Some(80_000_000),
        budget_shared: false,
        budget_campaigns: 1,
        bidding_strategy: "MAXIMIZE_CONVERSIONS".into(),
    }]);
    assert_eq!(
        keysend::optimize::execute::run(&settings, &store, &google, proposal, day, 80_000_000, now)
            .await
            .unwrap(),
        keysend::optimize::execute::Ran::Applied
    );
    // Ten days pass, but no day after the change was collected.
    settings.services.test_now = Some(settings.now() + chrono::Duration::days(10));
    cycle::evaluate(&settings, &store, AD, now + 10 * 86_400)
        .await
        .unwrap();
    let outcome = store
        .optimization_events(Some(AD), Some(proposal), 50)
        .await
        .unwrap()
        .into_iter()
        .find(|event| event.kind == "outcome")
        .expect("the evaluation records the Outcome");
    assert_eq!(outcome.detail["state"], "awaiting_data");
    assert_eq!(outcome.detail["observed"]["settled_days"], 0);
}
