use super::*;

use keysend::workspace::measurement::{self, Activity, Operation};

async fn activity(f: &Fixture, kind: Activity) -> i64 {
    let signed = signed(f).await;
    WorkspaceStore::new(&f.ctx.db, &signed.workspace)
        .activity_count(kind)
        .await
        .unwrap()
}

#[tokio::test]
#[serial]
async fn a_workspace_is_active_when_its_user_opens_an_ad_account_page() {
    let f = pages::normal_day().await;
    f.page("/ad-accounts/4829153307").await;
    assert_eq!(activity(&f, Activity::Viewed).await, 1);
}

#[tokio::test]
#[serial]
async fn a_daily_run_does_not_make_a_workspace_active() {
    let f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    worker::run(&f.ctx, &job).await.unwrap();
    assert_eq!(user_activity(&f).await, 0);
    assert!(activity(&f, Activity::Synced).await > 0);
}

#[tokio::test]
#[serial]
async fn an_export_is_recorded_when_the_pdf_is_delivered() {
    let f = pages::normal_day().await;
    let response = f
        .request
        .get("/ad-accounts/4829153307/export.pdf")
        .add_cookie(f.a.clone())
        .await;
    assert_eq!(response.status_code(), 200);
    assert!(response.as_bytes().starts_with(b"%PDF"));
    assert_eq!(activity(&f, Activity::Exported).await, 1);
    assert_eq!(operation_successes(&f, Operation::Export).await, 1);
}

#[tokio::test]
#[serial]
async fn a_failed_collection_keeps_the_last_successful_collection_time() {
    let f = Fixture::monitoring().await;
    f.prepare_monitoring().await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    f.ctx
        .db
        .execute_unprepared("UPDATE account_history SET succeeded_at=123")
        .await
        .unwrap();
    let before = store.history_state(AD).await.unwrap().unwrap().succeeded_at;
    assert!(before.is_some());
    f.calls.lock().unwrap().google.revoked = true;
    assert!(
        keysend::google_ads::collect_history(&Settings::load(&f.ctx).unwrap(), &store, AD)
            .await
            .is_err()
    );
    assert_eq!(
        store.history_state(AD).await.unwrap().unwrap().succeeded_at,
        before
    );
}

#[tokio::test]
#[serial]
async fn removing_a_workspace_deletes_its_activity_and_subscription_changes() {
    let f = Fixture::monitoring().await;
    f.subscribe().await;
    f.page("/ad-accounts/4829153307").await;
    assert_eq!(activity(&f, Activity::Viewed).await, 1);
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    assert_eq!(store.subscription_changes().await.unwrap().len(), 1);
    let response = f
        .request
        .post(&format!("/remove/workspaces/{}", signed.workspace.id()))
        .add_header("origin", ORIGIN)
        .add_cookie(f.a.clone())
        .form(&json!({"confirm":"remove"}))
        .await;
    assert_eq!(response.status_code(), 303);
    for kind in Activity::ALL {
        assert_eq!(store.activity_count(kind).await.unwrap(), 0);
    }
    assert!(store.subscription_changes().await.unwrap().is_empty());
}

#[tokio::test]
#[serial]
async fn an_ai_request_records_its_tokens_and_a_failed_one_counts_as_failed() {
    let mut f = Fixture::monitoring().await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    update::generate(
        &Settings::load(&f.ctx).unwrap(),
        Some(&store),
        moved(),
        AllFollowAccount { judged: true },
    )
    .await
    .unwrap();
    let url = f.ctx.config.settings.as_ref().unwrap()["services"]["ai_url"]
        .as_str()
        .unwrap()
        .to_owned();
    f.ctx.config.settings.as_mut().unwrap()["services"]["ai_url"] =
        json!(url.replace("/commentary", "/missing"));
    update::generate(
        &Settings::load(&f.ctx).unwrap(),
        Some(&store),
        moved(),
        AllFollowAccount { judged: true },
    )
    .await
    .unwrap();
    let rows = measurement::operation_days(&f.ctx.db, Operation::AiUpdate)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!((row.succeeded, row.failed), (1, 1));
    assert_eq!((row.input_tokens, row.output_tokens), (Some(120), Some(30)));
    assert_eq!(row.model, "gpt-6.1-sol");
}

#[tokio::test]
#[serial]
async fn the_daily_business_snapshot_is_written_once_per_day() {
    let f = Fixture::monitoring().await;
    let now = keysend::settings::wall_clock();
    keysend::monitoring::schedule::snapshot_business(&f.ctx, now).await;
    let before = measurement::business_days(&f.ctx.db).await.unwrap();
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].paying, 0);
    f.subscribe().await;
    keysend::monitoring::schedule::snapshot_business(&f.ctx, now).await;
    assert_eq!(measurement::business_days(&f.ctx.db).await.unwrap(), before);
    keysend::monitoring::schedule::snapshot_business(&f.ctx, now + chrono::Duration::days(1)).await;
    let after = measurement::business_days(&f.ctx.db).await.unwrap();
    assert_eq!(after.len(), 2);
    assert_eq!((after[1].paying, after[1].mrr_cents), (1, 4900));
}

async fn user_activity(f: &Fixture) -> i64 {
    let mut total = 0;
    for kind in Activity::ALL.into_iter().filter(|kind| kind.by_user()) {
        total += activity(f, kind).await;
    }
    total
}

async fn operation_successes(f: &Fixture, op: Operation) -> i64 {
    measurement::operation_days(&f.ctx.db, op)
        .await
        .unwrap()
        .iter()
        .map(|day| day.succeeded)
        .sum()
}

#[tokio::test]
#[serial]
async fn the_user_can_open_pages_when_measurement_storage_is_unavailable() {
    let f = pages::normal_day().await;
    f.ctx
        .db
        .execute_unprepared("DROP TABLE workspace_activity")
        .await
        .unwrap();
    f.page("/ad-accounts/4829153307").await;
    let response = f
        .request
        .get("/ad-accounts/4829153307/export.pdf")
        .add_cookie(f.a.clone())
        .await;
    assert_eq!(response.status_code(), 200);
    assert!(response.as_bytes().starts_with(b"%PDF"));
}

#[tokio::test]
#[serial]
async fn daily_measurement_expires_after_twenty_four_months() {
    let f = Fixture::monitoring().await;
    let signed = signed(&f).await;
    let oldest = today().checked_sub_months(chrono::Months::new(24)).unwrap();
    let boundary = day_number(oldest);
    for day in [boundary - 1, boundary] {
        f.ctx
            .db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "INSERT INTO workspace_activity VALUES (?,?,'viewed',1)",
                vec![signed.workspace.id().into(), day.into()],
            ))
            .await
            .unwrap();
        f.ctx
            .db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "INSERT INTO operation_days VALUES (?,'export','',1,0,0,NULL,NULL)",
                vec![day.into()],
            ))
            .await
            .unwrap();
        measurement::snapshot(
            &f.ctx.db,
            keysend::workspace::history::date_of(day)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc(),
        )
        .await
        .unwrap();
    }
    keysend::monitoring::retention::run(&f.ctx).await.unwrap();
    assert_eq!(activity(&f, Activity::Viewed).await, 1);
    let operations = measurement::operation_days(&f.ctx.db, Operation::Export)
        .await
        .unwrap();
    assert_eq!(operations.len(), 1);
    assert_eq!(operations[0].day, boundary);
    let snapshots = measurement::business_days(&f.ctx.db).await.unwrap();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].day, boundary);
}

fn moved() -> detection::Detection {
    detection::Detection {
        day: day_number(today()),
        currency: "EUR".into(),
        metrics: keysend::monitoring::metrics::WindowMetrics {
            window: Window::SevenDays,
            end_day: day_number(end()),
            current: Default::default(),
            previous: Default::default(),
        },
        movements: vec![detection::Movement {
            metric: detection::MetricName::Spend,
            current: 140.0,
            previous: 70.0,
            normal_variation: 10.0,
        }],
        yesterday: vec![],
        campaigns: vec![],
        judge_by_conversions: true,
        causes: vec![],
    }
}

#[tokio::test]
#[serial]
async fn an_ai_failure_keeps_received_usage_and_missing_usage_stays_unknown() {
    let mut f = Fixture::monitoring().await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    for response in [
        json!({"output":[{"type":"message","content":[{"type":"output_text","text":"{not JSON"}]}], "usage":{"input_tokens":12,"output_tokens":3}}),
        json!({"output":[{"type":"message","content":[{"type":"output_text","text":"{}"}]}]}),
    ] {
        f.calls.lock().unwrap().ai_response = Some(response);
        update::generate(
            &Settings::load(&f.ctx).unwrap(),
            Some(&store),
            moved(),
            AllFollowAccount { judged: true },
        )
        .await
        .unwrap();
    }
    let rows = measurement::operation_days(&f.ctx.db, Operation::AiUpdate)
        .await
        .unwrap();
    assert_eq!((rows[0].succeeded, rows[0].failed), (1, 1));
    assert_eq!(
        (rows[0].input_tokens, rows[0].output_tokens),
        (Some(12), Some(3))
    );
    f.ctx.config.settings.as_mut().unwrap()["services"]["ai_model"] = json!("model-without-usage");
    update::generate(
        &Settings::load(&f.ctx).unwrap(),
        Some(&store),
        moved(),
        AllFollowAccount { judged: true },
    )
    .await
    .unwrap();
    let rows = measurement::operation_days(&f.ctx.db, Operation::AiUpdate)
        .await
        .unwrap();
    let unknown = rows
        .iter()
        .find(|row| row.model == "model-without-usage")
        .unwrap();
    assert_eq!((unknown.input_tokens, unknown.output_tokens), (None, None));
}

#[tokio::test]
#[serial]
async fn a_recording_failure_does_not_change_access_or_an_ai_answer() {
    let f = Fixture::monitoring().await;
    f.ctx
        .db
        .execute_unprepared("DROP TABLE subscription_changes")
        .await
        .unwrap();
    f.subscribe().await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    assert!(store.subscription_active().await.unwrap());
    f.ctx
        .db
        .execute_unprepared("DROP TABLE operation_days")
        .await
        .unwrap();
    let update = update::generate(
        &Settings::load(&f.ctx).unwrap(),
        Some(&store),
        moved(),
        AllFollowAccount { judged: true },
    )
    .await
    .unwrap();
    assert!(!update.written_without_ai);
}
