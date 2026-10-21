use super::*;
use keysend::{
    agent::data,
    monitoring::audit::Counts,
    optimize::{
        ads::Simulated,
        cycle, execute,
        policy::{Live, Policy, Standing},
        propose::Decision,
        shadow::Evaluation,
        signals::{Kind, Severity, Signal},
        Action, Approval, Draft, Mode,
    },
    settings::Settings,
    workspace::history::{date_of, day_number, AccountChange, CampaignDay},
};
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

const AD: &str = "4829153307";

fn cut(version: i64, now: i64) -> Draft {
    Draft {
        key: "adjust_campaign_budget:701".into(),
        action: Action::AdjustCampaignBudget {
            campaign_id: 701,
            budget_id: 9001,
            from_micros: 80_000_000,
            to_micros: 64_000_000,
        },
        signal: Signal {
            kind: Kind::CostAboveTarget,
            campaign_id: Some(701),
            campaign: Some("Search".into()),
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
        expires_at: now + 86400,
    }
}

async fn add_days(store: &WorkspaceStore<'_>, from: i64, to: i64) {
    let days: Vec<_> = (from..=to)
        .map(|day| CampaignDay {
            day: date_of(day).unwrap(),
            campaign_id: 701,
            device: 1,
            counts: Counts {
                impressions: 100.0,
                clicks: 10.0,
                cost: 10.0,
                conversions: 1.0,
                value: 5.0,
            },
        })
        .collect();
    store
        .save_history(
            AD,
            date_of(from).unwrap(),
            date_of(to).unwrap(),
            &[],
            &days,
            &[],
        )
        .await
        .unwrap();
}

#[tokio::test]
#[serial]
async fn an_applied_budget_cut_excludes_only_its_exact_account_change() {
    let f = Fixture::monitoring().await;
    f.prepare_monitoring().await;
    let signed = identity::authenticate(&f.ctx.db, f.a.value())
        .await
        .unwrap()
        .unwrap();
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let mut settings = Settings::load(&f.ctx).unwrap();
    settings.optimize.live = true;
    settings.optimize.max_mode = Mode::Autopilot;
    let now = settings.now().timestamp();
    f.ctx
        .db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "UPDATE workspace_trials SET started_at=? WHERE workspace_id=?",
            vec![now.into(), signed.workspace.id().into()],
        ))
        .await
        .unwrap();
    let day = day_number(settings.now().date_naive());
    let version = store
        .save_optimization_policy(
            AD,
            &Policy {
                mode: Mode::Recommend,
                observation_days: 7,
                ..Default::default()
            },
            Standing::Active,
            "user",
            now,
        )
        .await
        .unwrap();
    let draft = cut(version, now);
    let proposal = store.save_proposal(AD, &draft, now).await.unwrap().unwrap();
    assert!(store
        .approve_proposal(proposal, &draft.fingerprint(AD), now)
        .await
        .unwrap());
    let google = Simulated::new(vec![Live {
        campaign_id: 701,
        status: "ENABLED".into(),
        budget_id: Some(9001),
        budget_micros: Some(80_000_000),
        budget_shared: false,
        budget_campaigns: 1,
        bidding_strategy: "MAXIMIZE_CONVERSIONS".into(),
    }]);
    assert_eq!(
        execute::run(&settings, &store, &google, proposal, day, 80_000_000, now)
            .await
            .unwrap(),
        execute::Ran::Applied
    );
    let change = AccountChange {
        changed_at: settings.now().format("%Y-%m-%d %H:%M:%S").to_string(),
        resource_name: format!("customers/{AD}/changeEvents/own"),
        change_resource_name: Some(format!("customers/{AD}/campaignBudgets/9001")),
        resource_type: "CAMPAIGN_BUDGET".into(),
        operation: "UPDATE".into(),
        campaign_id: Some(701),
        changed_fields: "amount_micros".into(),
        old_value: json!({"campaignBudget":{"amountMicros":"80000000"}}).to_string(),
        new_value: json!({"campaignBudget":{"amountMicros":"64000000"}}).to_string(),
    };
    store
        .save_history(
            AD,
            date_of(day).unwrap(),
            date_of(day).unwrap(),
            &[],
            &[],
            std::slice::from_ref(&change),
        )
        .await
        .unwrap();
    add_days(&store, day + 1, day + 9).await;
    settings.services.test_now = Some(settings.now() + chrono::Duration::days(10));
    cycle::evaluate(&settings, &store, AD, now + 10 * 86400)
        .await
        .unwrap();
    let events = store
        .optimization_events(Some(AD), Some(proposal), 50)
        .await
        .unwrap();
    let outcome = events.iter().find(|event| event.kind == "outcome").unwrap();
    assert_eq!(
        outcome.detail["state"], "inconclusive",
        "an exact own change must not confound its Outcome"
    );
    let external = AccountChange {
        resource_name: format!("customers/{AD}/changeEvents/external"),
        campaign_id: None,
        change_resource_name: Some(format!("customers/{AD}/campaignBudgets/9002")),
        ..change
    };
    store
        .save_history(
            AD,
            date_of(day).unwrap(),
            date_of(day).unwrap(),
            &[],
            &[],
            &[external],
        )
        .await
        .unwrap();
    cycle::evaluate(&settings, &store, AD, now + 11 * 86400)
        .await
        .unwrap();
    let events = store
        .optimization_events(Some(AD), Some(proposal), 50)
        .await
        .unwrap();
    assert_eq!(
        events
            .iter()
            .find(|event| event.kind == "outcome")
            .unwrap()
            .detail["state"],
        "manual_review"
    );
}

#[tokio::test]
#[serial]
async fn a_shadow_stops_recording_after_its_observation_window_is_covered() {
    let f = Fixture::monitoring().await;
    f.prepare_monitoring().await;
    let signed = identity::authenticate(&f.ctx.db, f.a.value())
        .await
        .unwrap()
        .unwrap();
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let mut settings = Settings::load(&f.ctx).unwrap();
    let now = settings.now().timestamp();
    f.ctx
        .db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "UPDATE workspace_trials SET started_at=? WHERE workspace_id=?",
            vec![now.into(), signed.workspace.id().into()],
        ))
        .await
        .unwrap();
    let day = day_number(settings.now().date_naive());
    let policy = Policy {
        mode: Mode::Shadow,
        observation_days: 7,
        ..Default::default()
    };
    let version = store
        .save_optimization_policy(AD, &policy, Standing::Active, "shadow_consent", now)
        .await
        .unwrap();
    let account = data::load(&store, AD, day - 1).await.unwrap();
    let evaluation = Evaluation::new(
        &account,
        &policy,
        version,
        day,
        now,
        vec![Decision::Propose(Box::new(cut(version, now)))],
    );
    store.save_shadow(AD, &evaluation).await.unwrap();
    let id = store.shadows(Some(AD), 1).await.unwrap()[0].id;
    for offset in [4, 5, 10, 11, 12] {
        add_days(&store, day + 1, day + offset - 1).await;
        settings.services.test_now = chrono::DateTime::from_timestamp(now + offset * 86400, 0);
        cycle::evaluate(&settings, &store, AD, now + offset * 86400)
            .await
            .unwrap();
        let events = store.shadow_events(id).await.unwrap();
        if offset == 5 {
            assert_eq!(events.len(), 2, "incomplete observations can gain coverage");
        }
    }
    let events = store.shadow_events(id).await.unwrap();
    assert_eq!(
        events.len(),
        3,
        "the completed window must stop appending observations"
    );
    assert_eq!(events[0].detail["observations"][0]["settled_days"], 7);
    let completed_at = events[0].at;
    for index in 0..105 {
        store
            .record_shadow_event(
                id,
                "feedback",
                &json!({"item":0,"feedback":"useful","note":index.to_string()}),
                now + 12 * 86400,
            )
            .await
            .unwrap();
    }
    add_days(&store, day + 1, day + 12).await;
    settings.services.test_now = chrono::DateTime::from_timestamp(now + 13 * 86400, 0);
    cycle::evaluate(&settings, &store, AD, now + 13 * 86400)
        .await
        .unwrap();
    assert_eq!(
        store
            .latest_shadow_observation(id)
            .await
            .unwrap()
            .unwrap()
            .at,
        completed_at,
        "feedback cannot make a completed window start recording again"
    );
    let page = f
        .request
        .get(&format!("/optimize/shadows/{id}"))
        .add_cookie(f.a.clone())
        .await;
    assert_eq!(page.status_code(), 200);
    assert!(
        page.text().contains("Days with data: 7 of 7"),
        "completed observation remains visible after feedback"
    );
}

#[tokio::test]
#[serial]
async fn expired_shadow_history_and_its_events_are_removed_only_for_the_ad_account() {
    let f = Fixture::monitoring().await;
    f.prepare_monitoring().await;
    let signed = identity::authenticate(&f.ctx.db, f.a.value())
        .await
        .unwrap()
        .unwrap();
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let settings = Settings::load(&f.ctx).unwrap();
    let now = settings.now().timestamp();
    let day = day_number(settings.now().date_naive());
    let oldest = day - 50;
    let policy = Policy {
        mode: Mode::Shadow,
        ..Default::default()
    };
    let version = store
        .save_optimization_policy(AD, &policy, Standing::Active, "shadow_consent", now)
        .await
        .unwrap();
    let mut account = data::load(&store, AD, day - 1).await.unwrap();
    let old = Evaluation::new(
        &account,
        &policy,
        version,
        day,
        now,
        vec![Decision::Propose(Box::new(cut(version, now)))],
    );
    store.save_shadow(AD, &old).await.unwrap();
    let old_id = store.shadows(Some(AD), 1).await.unwrap()[0].id;
    store
        .record_shadow_event(
            old_id,
            "feedback",
            &json!({"item":0,"feedback":"useful"}),
            now,
        )
        .await
        .unwrap();
    f.ctx.db.execute_raw(Statement::from_sql_and_values(DatabaseBackend::Sqlite,
        "INSERT INTO ad_accounts(id,workspace_id,customer_id,google_connection_id,name) SELECT 'shadow-second',workspace_id,'1111111111',google_connection_id,name FROM ad_accounts WHERE workspace_id=? AND customer_id=?",
        vec![signed.workspace.id().into(), AD.into()])).await.unwrap();
    let version2 = store
        .save_optimization_policy(
            "1111111111",
            &policy,
            Standing::Active,
            "shadow_consent",
            now,
        )
        .await
        .unwrap();
    let mut second = old.clone();
    second.policy_version = version2;
    store.save_shadow("1111111111", &second).await.unwrap();
    let second_id = store.shadows(Some("1111111111"), 1).await.unwrap()[0].id;
    account.days.retain(|row| row.day >= oldest);
    account.first_day = Some(oldest);
    let mut fresh = Evaluation::new(&account, &policy, version, day, now + 1, vec![]);
    fresh.data_day += 1;
    store.save_shadow(AD, &fresh).await.unwrap();
    let fresh_id = store.shadows(Some(AD), 1).await.unwrap()[0].id;
    let other = identity::authenticate(&f.ctx.db, f.b.value())
        .await
        .unwrap()
        .unwrap();
    WorkspaceStore::new(&f.ctx.db, &other.workspace)
        .remove_history_before(
            AD,
            oldest,
            &format!("{} 00:00:00", date_of(oldest).unwrap()),
        )
        .await
        .unwrap();
    assert!(store.shadow(old_id).await.unwrap().is_some());
    store
        .remove_history_before(
            AD,
            oldest,
            &format!("{} 00:00:00", date_of(oldest).unwrap()),
        )
        .await
        .unwrap();
    assert!(
        store.shadow(old_id).await.unwrap().is_none(),
        "snapshots cannot retain expired raw history"
    );
    assert!(store.shadow(fresh_id).await.unwrap().is_some());
    assert!(store.shadow(second_id).await.unwrap().is_some());
    let row = f
        .ctx
        .db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "SELECT COUNT(*) AS n FROM optimization_shadow_events WHERE shadow_id=?",
            [old_id.into()],
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.try_get::<i64>("", "n").unwrap(),
        0,
        "retention cascades to Shadow events"
    );
}

#[tokio::test]
#[serial]
async fn the_daily_run_keeps_the_changed_resource_separate_from_the_account_change_id() {
    let f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    let target = format!("customers/{AD}/campaignBudgets/9001");
    let event = format!("customers/{AD}/changeEvents/own");
    f.calls.lock().unwrap().google.monitoring_changes = vec![json!({"changeEvent": {
        "changeDateTime":"2026-10-11 12:00:00", "changeResourceType":"CAMPAIGN_BUDGET",
        "resourceChangeOperation":"UPDATE", "resourceName":event, "changeResourceName":target,
        "campaign":format!("customers/{AD}/campaigns/701"), "changedFields":"amountMicros",
        "oldResource":{"campaignBudget":{"amountMicros":"80000000"}},
        "newResource":{"campaignBudget":{"amountMicros":"64000000"}}
    }})];
    keysend::monitoring::worker::run(&f.ctx, &job)
        .await
        .unwrap();
    let signed = identity::authenticate(&f.ctx.db, f.a.value())
        .await
        .unwrap()
        .unwrap();
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let date = chrono::NaiveDate::from_ymd_opt(2026, 10, 11).unwrap();
    let changes = store.account_changes(AD, date, date).await.unwrap();
    let change = changes
        .iter()
        .find(|change| change.resource_name == event)
        .unwrap();
    assert_eq!(
        change.change_resource_name.as_deref(),
        Some(target.as_str())
    );
    assert!(f
        .calls
        .lock()
        .unwrap()
        .google
        .requests
        .iter()
        .any(|(_, request)| {
            request["query"]
                .as_str()
                .is_some_and(|query| query.contains("change_event.change_resource_name"))
        }));
}
