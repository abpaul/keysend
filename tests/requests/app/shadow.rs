use super::*;
use axum::http::StatusCode;
use keysend::{
    optimize::{
        cycle,
        policy::{Policy, Standing},
        shadow::Feedback,
        Mode, ProposalState,
    },
    settings::Settings,
};

const AD: &str = "4829153307";

#[tokio::test]
#[serial]
async fn a_shadow_evaluation_records_why_a_missing_target_cannot_justify_a_change() {
    let f = Fixture::monitoring().await;
    f.prepare_monitoring().await;
    let signed = identity::authenticate(&f.ctx.db, f.a.value())
        .await
        .unwrap()
        .unwrap();
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let settings = Settings::load(&f.ctx).unwrap();
    let now = settings.now().timestamp();
    store
        .save_optimization_policy(
            AD,
            &Policy {
                mode: Mode::Shadow,
                ..Default::default()
            },
            Standing::Active,
            "shadow_consent",
            now,
        )
        .await
        .unwrap();
    assert!(cycle::evaluate(&settings, &store, AD, now)
        .await
        .unwrap()
        .is_empty());
    let shadow = store.shadows(Some(AD), 1).await.unwrap().remove(0);
    assert!(shadow.evaluation.decisions.is_empty());
    assert_eq!(
        shadow.evaluation.abstention,
        Some(keysend::optimize::shadow::Abstention::MissingCommercialTarget)
    );
    assert!(f.calls.lock().unwrap().google.mutations.is_empty());
}

#[tokio::test]
#[serial]
async fn an_ad_account_enters_shadow_only_with_explicit_consent() {
    let f = Fixture::monitoring().await;
    f.prepare_monitoring().await;
    let signed = identity::authenticate(&f.ctx.db, f.a.value())
        .await
        .unwrap()
        .unwrap();
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let mut form = vec![
        ("mode", "shadow"),
        ("max_decrease_percent", "20"),
        ("max_increase_percent", "10"),
        ("envelope", "1000"),
        ("max_daily_increase", "50"),
        ("changes_per_day", "3"),
        ("cooldown_days", "7"),
        ("min_conversions", "10"),
        ("target_cost", "5"),
    ];
    let path = format!("/ad-accounts/{AD}/optimize");
    assert_eq!(
        f.request
            .post(&path)
            .add_cookie(f.a.clone())
            .add_header("origin", ORIGIN)
            .form(&form)
            .await
            .status_code(),
        422
    );
    assert_eq!(
        store.optimization_policy(AD).await.unwrap().policy.mode,
        Mode::Investigate
    );
    form.push(("shadow_consent", "yes"));
    assert_eq!(
        f.request
            .post(&path)
            .add_cookie(f.a.clone())
            .add_header("origin", ORIGIN)
            .form(&form)
            .await
            .status_code(),
        303
    );
    let current = store.optimization_policy(AD).await.unwrap();
    assert_eq!(current.policy.mode, Mode::Shadow);
    assert_eq!(current.reason, "shadow_consent");
    assert!(f.calls.lock().unwrap().google.mutations.is_empty());
}

#[tokio::test]
#[serial]
async fn a_shadow_evaluation_records_the_decisions_but_sends_no_mutation() {
    let f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    let signed = identity::authenticate(&f.ctx.db, f.a.value())
        .await
        .unwrap()
        .unwrap();
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let mut settings = Settings::load(&f.ctx).unwrap();
    settings.optimize.live = true;
    settings.optimize.max_mode = keysend::optimize::Mode::Autopilot;
    settings.services.ai_prices.clear();
    settings.openai_key = None;
    let now = settings.now().timestamp();
    let policy = Policy {
        mode: Mode::Shadow,
        target_cost_micros: Some(5_000_000),
        ..Default::default()
    };
    store
        .save_optimization_policy(AD, &policy, Standing::Active, "shadow_consent", now)
        .await
        .unwrap();
    f.calls.lock().unwrap().google.monitoring_campaigns = Some(vec![json!({
        "campaign":{"id":"701","name":"Search - Real","status":"ENABLED","advertisingChannelType":"SEARCH","biddingStrategyType":"MAXIMIZE_CONVERSIONS"},
        "campaignBudget":{"id":"9001","amountMicros":"10000000","explicitlyShared":false,"referenceCount":"1"}
    })]);
    keysend::monitoring::worker::run(&f.ctx, &job)
        .await
        .unwrap();
    let shadows = store.shadows(Some(AD), 10).await.unwrap();
    assert_eq!(shadows.len(), 1);
    let shadow = &shadows[0];
    assert!(!shadow.evaluation.decisions.is_empty());
    assert!(shadow
        .evaluation
        .decisions
        .iter()
        .any(|decision| matches!(decision, keysend::optimize::propose::Decision::Propose(_))));
    let mut subsequent = keysend::agent::data::load(&store, AD, shadow.evaluation.day - 1)
        .await
        .unwrap();
    for row in &mut subsequent.days {
        row.metrics.spend = 999.0;
    }
    for offset in [1, 3] {
        subsequent
            .days
            .push(keysend::monitoring::metrics::CampaignDay {
                day: shadow.evaluation.day + offset,
                campaign_id: 701,
                metrics: keysend::monitoring::metrics::Metrics {
                    spend: 5.0,
                    conversions: Some(1.0),
                    ..Default::default()
                },
            });
    }
    subsequent.end_day = shadow.evaluation.day + 5;
    let observation = shadow.evaluation.observe(&subsequent).unwrap().remove(0);
    assert_eq!(observation.before_spend, 140.0);
    assert_eq!(observation.before_days, 14);
    assert_eq!(observation.after_spend, 10.0);
    assert_eq!(observation.settled_days, 2);
    assert!(shadow.evaluation.snapshot["days"].as_array().unwrap().len() > 30);
    assert!(store
        .proposals(Some(AD), &ProposalState::ALL, 50)
        .await
        .unwrap()
        .is_empty());
    assert!(cycle::evaluate(&settings, &store, AD, now + 60)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(store.shadows(Some(AD), 10).await.unwrap().len(), 1);
    assert!(f.calls.lock().unwrap().google.mutations.is_empty());
    assert!(store
        .record_shadow_feedback(shadow.id, 0, Feedback::TooRisky, "Need lead quality", now)
        .await
        .unwrap());
    let events = store.shadow_events(shadow.id).await.unwrap();
    assert_eq!(events[0].detail["feedback"], "too_risky");
    let quality = store.decision_quality().await.unwrap();
    assert_eq!(quality.shadow_evaluations, 1);
    assert_eq!(quality.feedback_total, 1);
    assert_eq!(quality.consultant_overrides, 1);
    assert_eq!(quality.verified_executions, 0);
    let other = identity::authenticate(&f.ctx.db, f.b.value())
        .await
        .unwrap()
        .unwrap();
    let other_store = WorkspaceStore::new(&f.ctx.db, &other.workspace);
    assert!(other_store.shadow(shadow.id).await.unwrap().is_none());
    assert!(!other_store
        .record_shadow_feedback(shadow.id, 0, Feedback::Useful, "", now)
        .await
        .unwrap());
    let path = format!("/optimize/shadows/{}", shadow.id);
    assert_eq!(
        f.request
            .get(&path)
            .add_cookie(f.a.clone())
            .await
            .status_code(),
        StatusCode::OK
    );
    assert_eq!(
        f.request
            .get(&path)
            .add_cookie(f.b.clone())
            .await
            .status_code(),
        StatusCode::NOT_FOUND
    );
    let response = f
        .request
        .post(&format!("{path}/feedback"))
        .add_cookie(f.a.clone())
        .add_header("origin", ORIGIN)
        .form(&[
            ("item", "0"),
            ("feedback", "missing_context"),
            ("note", "Check the qualified leads"),
        ])
        .await;
    assert_eq!(response.status_code(), StatusCode::SEE_OTHER);
    assert!(f
        .request
        .get(&path)
        .add_cookie(f.a.clone())
        .await
        .text()
        .contains("Check the qualified leads"));
    for feedback in ["useful", "too_risky", "useful"] {
        let response = f
            .request
            .post(&format!("{path}/feedback"))
            .add_cookie(f.a.clone())
            .add_header("origin", ORIGIN)
            .form(&[("item", "0"), ("feedback", feedback), ("note", "")])
            .await;
        assert_eq!(response.status_code(), StatusCode::SEE_OTHER);
    }
    let quality = store.decision_quality().await.unwrap();
    assert_eq!(quality.useful_feedback, 1);
    assert_eq!(quality.consultant_overrides, 0);
}

#[tokio::test]
#[serial]
async fn an_approved_change_cannot_run_after_the_ad_account_enters_shadow() {
    use keysend::optimize::{
        signals::{Kind, Severity, Signal},
        Action, Approval, Draft,
    };
    let f = Fixture::monitoring().await;
    f.prepare_monitoring().await;
    let signed = identity::authenticate(&f.ctx.db, f.a.value())
        .await
        .unwrap()
        .unwrap();
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let mut settings = Settings::load(&f.ctx).unwrap();
    settings.optimize.live = true;
    settings.optimize.max_mode = keysend::optimize::Mode::Autopilot;
    let now = settings.now().timestamp();
    let version = store
        .save_optimization_policy(
            AD,
            &Policy {
                mode: Mode::Recommend,
                ..Default::default()
            },
            Standing::Active,
            "user",
            now,
        )
        .await
        .unwrap();
    let draft = Draft {
        key: "pause_campaign:701".into(),
        action: Action::PauseCampaign { campaign_id: 701 },
        signal: Signal {
            kind: Kind::SpendWithoutConversions,
            campaign_id: Some(701),
            campaign: Some("Search".into()),
            term: None,
            from_day: 0,
            to_day: 29,
            spend: 100.0,
            clicks: 30.0,
            conversions: 0.0,
            target: Some(25.0),
            chance: Some(0.01),
            lost_to_budget: None,
            daily_spend: Some(10.0),
            daily_budget: Some(80.0),
            data_day: 29,
            severity: Severity::High,
            caveats: vec![],
        },
        approval: Approval::Required,
        policy_version: version,
        reasons: vec![],
        created_at: now,
        expires_at: now + 86400,
    };
    let id = store.save_proposal(AD, &draft, now).await.unwrap().unwrap();
    assert!(store
        .approve_proposal(id, &draft.fingerprint(AD), now)
        .await
        .unwrap());
    store
        .save_optimization_policy(
            AD,
            &Policy {
                mode: Mode::Shadow,
                ..Default::default()
            },
            Standing::Active,
            "shadow_consent",
            now + 1,
        )
        .await
        .unwrap();
    assert_ne!(
        cycle::execute_one(&settings, &store, id, now + 2)
            .await
            .unwrap(),
        keysend::optimize::execute::Ran::Applied
    );
    assert!(f.calls.lock().unwrap().google.mutations.is_empty());
}
