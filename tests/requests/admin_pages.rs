use super::{
    admin::{fixture, signed},
    app::Fixture,
};
use axum_extra::extract::cookie::Cookie;
use keysend::{
    identity,
    settings::Settings,
    workspace::{measurement::Activity, WorkspaceStore},
};
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement, Value};
use serial_test::serial;

async fn sql(f: &Fixture, query: &str, values: Vec<Value>) {
    f.ctx
        .db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            query,
            values,
        ))
        .await
        .unwrap();
}
async fn workspace(f: &Fixture, email: &str) -> String {
    let token = identity::sign_in(&f.ctx.db, email, email, "Invented User", now(f), None)
        .await
        .unwrap();
    identity::authenticate(&f.ctx.db, &token)
        .await
        .unwrap()
        .unwrap()
        .workspace
        .id()
        .to_owned()
}
fn now(f: &Fixture) -> i64 {
    Settings::load(&f.ctx).unwrap().now().timestamp()
}
async fn subscription(f: &Fixture, email: &str, status: &str, end: i64, scheduled: bool) -> String {
    let id = workspace(f, email).await;
    sql(f,"INSERT INTO subscriptions(workspace_id,stripe_subscription_id,created_at,status,period_start,period_end,cancel_at_period_end) VALUES (?,'sub_private_fixture',0,?,0,?,?)",vec![id.clone().into(),status.into(),end.into(),scheduled.into()]).await;
    id
}
async fn page(f: &Fixture, cookie: &Cookie<'static>, path: &str) -> String {
    let response = f.request.get(path).add_cookie(cookie.clone()).await;
    assert_eq!(response.status_code(), 200, "{}", response.text());
    assert_eq!(response.header("cache-control"), "no-store");
    response.text()
}
fn figure(html: &str, label: &str, value: &str) {
    let mut tag = false;
    let text = html
        .chars()
        .map(|c| match c {
            '<' => {
                tag = true;
                ' '
            }
            '>' => {
                tag = false;
                ' '
            }
            _ if tag => ' ',
            _ => c,
        })
        .collect::<String>();
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        text.contains(&format!("{label} {value}")),
        "expected {label} {value}; got {text}"
    );
}

#[tokio::test]
#[serial]
async fn mrr_counts_only_paying_workspaces_at_49_usd() {
    let f = super::admin::fixture_with_prices().await;
    f.subscribe().await;
    let c = signed(&f).await;
    for (email, status, end) in [
        ("trial@example.com", "trialing", now(&f) + 86400),
        ("due@example.com", "past_due", now(&f) + 86400),
        ("cancel@example.com", "canceled", now(&f) + 86400),
        ("expired@example.com", "active", now(&f)),
    ] {
        subscription(&f, email, status, end, false).await;
    }
    let html = page(&f, &c, "/admin/revenue").await;
    figure(&html, "MRR", "$49.00");
    figure(&html, "ARR", "$588.00");
    figure(&html, "Paying Workspaces", "1");
    let overview = page(&f, &c, "/admin").await;
    figure(&overview, "MRR", "$49.00");
    keysend::workspace::measurement::snapshot(&f.ctx.db, Settings::load(&f.ctx).unwrap().now())
        .await
        .unwrap();
    assert_eq!(
        keysend::workspace::measurement::business_days(&f.ctx.db)
            .await
            .unwrap()[0]
            .mrr_cents,
        4900
    );
}
#[tokio::test]
#[serial]
async fn a_trial_is_not_counted_as_paying() {
    let f = fixture().await;
    let c = signed(&f).await;
    let html = page(&f, &c, "/admin/revenue").await;
    figure(&html, "MRR", "$0.00");
    figure(&html, "In Trial", "2");
    figure(&html, "Paying Workspaces", "0");
}
#[tokio::test]
#[serial]
async fn a_scheduled_cancellation_is_shown_apart_from_a_completed_one() {
    let f = fixture().await;
    let c = signed(&f).await;
    subscription(&f, "scheduled@example.com", "active", now(&f) + 86400, true).await;
    subscription(
        &f,
        "cancelled@example.com",
        "canceled",
        now(&f) + 86400,
        false,
    )
    .await;
    let html = page(&f, &c, "/admin/revenue").await;
    figure(&html, "Scheduled cancellations", "1");
    figure(&html, "Completed cancellations", "1");
    figure(&html, "MRR", "$49.00");
}
#[tokio::test]
#[serial]
async fn active_workspaces_require_user_activity_not_background_runs() {
    let f = fixture().await;
    let c = signed(&f).await;
    for (cookie, kind) in [(&f.a, Activity::Synced), (&f.b, Activity::Viewed)] {
        let user = identity::authenticate(&f.ctx.db, cookie.value())
            .await
            .unwrap()
            .unwrap();
        WorkspaceStore::new(&f.ctx.db, &user.workspace)
            .record_activity(kind)
            .await;
    }
    // The fixture's product clock predates machine-time measurement recording.
    sql(
        &f,
        "UPDATE workspace_activity SET day=?",
        vec![(now(&f) / 86400).into()],
    )
    .await;
    let html = page(&f, &c, "/admin/usage").await;
    figure(&html, "Active Workspaces · 30 days", "1");
    figure(&html, "Active Workspaces · 7 days", "1");
}
#[tokio::test]
#[serial]
async fn the_overview_explains_empty_data_without_inventing_history() {
    let f = fixture().await;
    let c = signed(&f).await;
    let html = page(&f, &c, "/admin").await;
    assert!(html.contains("Recording started on"));
    assert!(html.contains("trends appear after the first daily snapshot."));
    figure(&html, "Trial-to-paid", "Not available");
    assert!(!html.contains("+100%"));
}
#[tokio::test]
#[serial]
async fn customers_can_be_searched_filtered_sorted_and_paged() {
    let f = fixture().await;
    let c = signed(&f).await;
    let special = subscription(
        &f,
        "percent%'_@example.com",
        "active",
        now(&f) + 86400,
        false,
    )
    .await;
    for n in 0..52 {
        let id = workspace(&f, &format!("invented{n:02}@example.com")).await;
        sql(
            &f,
            "UPDATE workspaces SET created_at=? WHERE id=?",
            vec![(now(&f) - n * 60).into(), id.into()],
        )
        .await;
    }
    let html = page(&f, &c, "/admin/customers?q=%25%27_").await;
    assert!(html.contains(&special));
    assert!(!html.contains("a@example.com"));
    let html = page(&f, &c, "/admin/customers?status=paying&sort=activity").await;
    assert!(html.contains(&special));
    assert!(!html.contains("invented00@example.com"));
    let html = page(&f, &c, "/admin/customers?sort=created&page=2").await;
    assert!(html.contains("Page 2"));
    assert!(html.contains("Previous"));
    assert!(html.contains("invented47@example.com"));
    assert!(html.contains("invented51@example.com"));
    assert!(!html.contains("invented46@example.com"));
    let first = page(&f, &c, "/admin/customers?sort=created&page=1").await;
    assert!(first.contains("invented46@example.com"));
    assert!(!first.contains("invented47@example.com"));
    let html = page(&f, &c, "/admin/customers?page=9999").await;
    assert!(html.contains("No Workspaces match these filters."));
    let html = page(
        &f,
        &c,
        "/admin/customers?status=garbage&sort=garbage&page=bad",
    )
    .await;
    assert!(html.contains("Page 1"));
    for (email, age, activity_age) in [
        ("earlier@sorting.example.com", 2, 0),
        ("later@sorting.example.com", 1, 1),
    ] {
        let id = workspace(&f, email).await;
        sql(
            &f,
            "UPDATE workspaces SET created_at=? WHERE id=?",
            vec![(now(&f) - age * 86400).into(), id.clone().into()],
        )
        .await;
        sql(
            &f,
            "INSERT INTO workspace_activity VALUES (?1,?2,'viewed',1)",
            vec![id.into(), (now(&f) / 86400 - activity_age).into()],
        )
        .await;
    }
    let created = page(
        &f,
        &c,
        "/admin/customers?q=sorting.example.com&sort=created",
    )
    .await;
    assert!(
        created.find("later@sorting.example.com").unwrap()
            < created.find("earlier@sorting.example.com").unwrap()
    );
    let activity = page(
        &f,
        &c,
        "/admin/customers?q=sorting.example.com&sort=activity",
    )
    .await;
    assert!(
        activity.find("earlier@sorting.example.com").unwrap()
            < activity.find("later@sorting.example.com").unwrap()
    );
    let html = page(&f, &c, &format!("/admin/customers?q={special}")).await;
    assert!(html.contains(&special));
}
#[tokio::test]
#[serial]
async fn a_customer_page_shows_operations_but_no_tokens_or_campaign_data() {
    let f = fixture().await;
    let c = signed(&f).await;
    let id = identity::authenticate(&f.ctx.db, f.a.value())
        .await
        .unwrap()
        .unwrap()
        .workspace
        .id()
        .to_owned();
    sql(&f,"INSERT INTO subscriptions(workspace_id,stripe_subscription_id,created_at,status,period_start,period_end) VALUES (?,'sub_do_not_show',0,'active',0,9999999999)",vec![id.clone().into()]).await;
    seed_assessment(&f).await;
    let html = page(&f, &c, &format!("/admin/customers/{id}")).await;
    assert!(html.contains("Northwind Dental"));
    assert!(html.contains("Last success"));
    assert!(html.contains("Assessment"));
    for secret in [
        "never-send-this-refresh-secret",
        "sub_do_not_show",
        "private-campaign-words",
    ] {
        assert!(!html.contains(secret));
    }
    assert_eq!(
        f.request
            .get("/admin/customers/missing")
            .add_cookie(c)
            .await
            .status_code(),
        404
    );
}
#[tokio::test]
#[serial]
async fn a_failed_collection_and_a_stale_ad_account_appear_on_operations() {
    let f = fixture().await;
    let c = signed(&f).await;
    let user = identity::authenticate(&f.ctx.db, f.a.value())
        .await
        .unwrap()
        .unwrap();
    WorkspaceStore::new(&f.ctx.db, &user.workspace)
        .history_failed("4829153307", "Collection unavailable")
        .await
        .unwrap();
    let html = page(&f, &c, "/admin/operations").await;
    assert!(html.contains("Collection unavailable"));
    assert!(html.contains("Northwind Dental"));
    assert!(html.contains("Stale Ad Accounts"));
}
#[tokio::test]
#[serial]
async fn an_operator_sees_only_actionable_workspaces_needing_attention() {
    let (f, cookie, _) = invented_dataset().await;
    let html = page(&f, &cookie, "/admin").await;
    let attention = html
        .split_once("Workspaces needing attention")
        .unwrap()
        .1
        .split_once("Latest operational failures")
        .unwrap()
        .0;
    assert!(
        attention.contains("user03@example.com"),
        "paying Workspace with a stale Ad Account is actionable"
    );
    for email in [
        "user08@example.com",
        "user09@example.com",
        "user10@example.com",
        "user11@example.com",
        "user12@example.com",
    ] {
        assert!(
            !attention.contains(email),
            "ended or cancelled Workspace must not need attention: {email}"
        );
    }
    assert!(html.contains("Stale Ad Account"));
    assert!(html.contains("Why"));
}

#[tokio::test]
#[serial]
async fn an_operator_reads_distinct_activation_workspaces_per_iso_week() {
    let f = fixture().await;
    let c = signed(&f).await;
    sql(&f, "UPDATE workspaces SET created_at=NULL", vec![]).await;
    let id = workspace(&f, "weekly@example.com").await;
    sql(
        &f,
        "UPDATE workspaces SET created_at=? WHERE id=?",
        vec![(now(&f) - 86400).into(), id.clone().into()],
    )
    .await;
    for offset in [-1, 0, 1, 7, 34, 35] {
        for kind in ["connected", "synced", "assessed", "exported"] {
            sql(
                &f,
                "INSERT INTO workspace_activity VALUES (?,?,?,3)",
                vec![
                    id.clone().into(),
                    (now(&f) / 86400 - offset).into(),
                    kind.into(),
                ],
            )
            .await;
        }
    }
    let html = page(&f, &c, "/admin").await;
    figure(&html, "2026-W41", "1 1 1 1 1");
    figure(&html, "2026-W40", "0 1 1 1 1");
    figure(&html, "2026-W37", "0 1 1 1 1");
    assert!(!html.contains("2026-W36"));
    assert!(!html.contains("2026-W42"));
    assert!(html.contains("Exported"));
    assert!(html.contains("2 older Workspaces have no recorded date"));
}

#[tokio::test]
#[serial]
async fn an_operator_reads_operation_and_queue_labels_and_user_email_links() {
    let f = fixture().await;
    let c = signed(&f).await;
    let user = identity::authenticate(&f.ctx.db, f.a.value())
        .await
        .unwrap()
        .unwrap();
    WorkspaceStore::new(&f.ctx.db, &user.workspace)
        .history_failed("4829153307", "Collection unavailable")
        .await
        .unwrap();
    for op in keysend::workspace::measurement::Operation::ALL {
        sql(
            &f,
            "INSERT INTO operation_days VALUES (?,?,'',1,0,100,0,0)",
            vec![(now(&f) / 86400).into(), op.as_str().into()],
        )
        .await;
    }
    queue(&f).await;
    for worker in ["EmailWorker", "TranslationWorker", "RetentionWorker"] {
        sql(&f, r#"INSERT INTO sqlt_loco_queue(id,name,task_data,status,run_at) VALUES (?,?,'{"error":"Invented failure"}','failed',CURRENT_TIMESTAMP)"#, vec![worker.into(), worker.into()]).await;
    }
    let html = page(&f, &c, "/admin/operations").await;
    for label in [
        "Daily sync",
        "Deep check",
        "Export",
        "Google Ads request",
        "AI: Update wording",
        "AI: Assessment and Recommendations",
        "AI: Business research",
        "AI: Translation",
        "AI: Investigation",
        "Daily run",
        "Email",
        "Translation",
        "Retention",
    ] {
        assert!(
            html.contains(label),
            "missing operation or worker label: {label}"
        );
    }
    for raw in [
        "MonitoringWorker",
        "EmailWorker",
        "TranslationWorker",
        "RetentionWorker",
        "ai_update",
        "google_request",
    ] {
        assert!(
            !html.contains(raw),
            "raw operation or worker name is visible: {raw}"
        );
    }
    let email_link = format!(
        "href=\"/admin/customers/{}\">a@example.com</a>",
        user.workspace.id()
    );
    assert!(html.contains(&email_link));
    assert!(page(&f, &c, "/admin").await.contains(&email_link));
    assert!(page(&f, &c, "/admin/customers").await.contains(&email_link));
}

#[tokio::test]
#[serial]
async fn an_operator_can_open_full_customer_and_failure_lists_from_the_overview() {
    let f = fixture().await;
    let c = signed(&f).await;
    for n in 0..6 {
        let id = workspace(&f, &format!("recent{n}@example.com")).await;
        sql(
            &f,
            "UPDATE workspaces SET created_at=? WHERE id=?",
            vec![(now(&f) - n * 86400).into(), id.into()],
        )
        .await;
    }
    queue(&f).await;
    sql(&f, "DELETE FROM sqlt_loco_queue", vec![]).await;
    for n in 0..6 {
        sql(&f, "INSERT INTO sqlt_loco_queue(id,name,task_data,status,run_at,updated_at) VALUES (?,'EmailWorker',?,'failed',CURRENT_TIMESTAMP,datetime('now',?))", vec![format!("failure{n}").into(), format!("{{\"error\":\"Invented failure {n}\"}}").into(), format!("-{n} minutes").into()]).await;
    }
    let html = page(&f, &c, "/admin").await;
    assert!(html.contains("recent0@example.com"));
    assert!(html.contains("recent4@example.com"));
    assert!(!html.contains("recent5@example.com"));
    assert!(html.contains("Invented failure 0"));
    assert!(html.contains("Invented failure 4"));
    assert!(!html.contains("Invented failure 5"));
    assert!(html.contains("href=\"/admin/customers\">All customers</a>"));
    assert!(html.contains("href=\"/admin/operations\">All operations</a>"));
    assert!(page(&f, &c, "/admin/customers")
        .await
        .contains("recent5@example.com"));
    assert!(page(&f, &c, "/admin/operations")
        .await
        .contains("Invented failure 5"));
}

async fn seed_assessment(f: &Fixture) {
    let signed = identity::authenticate(&f.ctx.db, f.a.value())
        .await
        .unwrap()
        .unwrap();
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let (mut check, recommendations) = keysend::monitoring::example::deep_check(now(f) / 86400 - 4);
    let mut assessment =
        keysend::monitoring::assessment::fallback(&keysend::monitoring::assessment::Judged {
            verdict: keysend::monitoring::assessment::Verdict::Steady,
            reasons: vec![],
        });
    assessment.headline = "private-campaign-words".into();
    check.assessment = Some(assessment);
    store
        .refresh_deep_check("4829153307", &check, &recommendations)
        .await
        .unwrap();
}

async fn queue(f: &Fixture) {
    // Loco owns this table; foreground request fixtures do not initialize a queue.
    sql(f,"CREATE TABLE IF NOT EXISTS sqlt_loco_queue(id TEXT,name TEXT,task_data JSON,status TEXT,run_at TIMESTAMP,interval INTEGER,created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,tags JSON,priority INTEGER DEFAULT 0)",vec![]).await;
    sql(f,"INSERT INTO sqlt_loco_queue(id,name,task_data,status,run_at) VALUES ('invented','MonitoringWorker','{\"error\":\"Invented job failed\",\"refresh_token\":\"never-show-queue-payload\"}','failed',CURRENT_TIMESTAMP)",vec![]).await;
}
#[tokio::test]
#[serial]
async fn failed_queue_jobs_appear_on_operations() {
    let f = fixture().await;
    let c = signed(&f).await;
    queue(&f).await;
    let html = page(&f, &c, "/admin/operations").await;
    figure(&html, "Failed background jobs · 7 days", "1");
    assert!(html.contains("Invented job failed"));
    assert!(!html.contains("never-show-queue-payload"));
}
#[tokio::test]
#[serial]
async fn ai_cost_is_estimated_from_configured_prices() {
    let f = super::admin::fixture_with_prices().await;
    let c = signed(&f).await;
    sql(&f,"INSERT INTO operation_days VALUES (?,'ai_investigation','invented-model',2,0,1000,1000000,500000)",vec![(now(&f)/86400).into()]).await;
    let html = page(&f, &c, "/admin/operations").await;
    assert!(html.contains("Estimate $4.00"));
    sql(
        &f,
        "UPDATE operation_days SET model='unpriced-model'",
        vec![],
    )
    .await;
    assert!(page(&f, &c, "/admin/operations")
        .await
        .contains("Not configured"));
    sql(
        &f,
        "UPDATE operation_days SET model='invented-model',input_tokens=NULL",
        vec![],
    )
    .await;
    let unknown = page(&f, &c, "/admin/operations").await;
    assert!(unknown.contains("Not available"));
    assert!(!unknown.contains("Estimate $0.00"));
}
#[tokio::test]
#[serial]
async fn every_admin_page_refuses_a_customer_session_and_no_session() {
    let f = fixture().await;
    for path in [
        "/admin/customers",
        "/admin/customers/missing",
        "/admin/revenue",
        "/admin/usage",
        "/admin/operations",
    ] {
        for cookie in [None, Some(f.a.clone())] {
            let mut request = f.request.get(path);
            if let Some(cookie) = cookie {
                request = request.add_cookie(cookie);
            }
            let response = request.await;
            assert_eq!(response.status_code(), 303, "{path}");
            assert_eq!(response.header("location"), "/admin");
        }
    }
}

pub(super) async fn invented_dataset() -> (Fixture, Cookie<'static>, String) {
    let f = super::admin::fixture_with_prices().await;
    let cookie = signed(&f).await;
    f.subscribe().await;
    sql(
        &f,
        "UPDATE seaql_migrations SET applied_at=? WHERE version='m20261020_000019_measurement'",
        vec![(now(&f) - 30 * 86400).into()],
    )
    .await;
    let primary = identity::authenticate(&f.ctx.db, f.a.value())
        .await
        .unwrap()
        .unwrap()
        .workspace
        .id()
        .to_owned();
    sql(
        &f,
        "UPDATE subscriptions SET cancel_at_period_end=1 WHERE workspace_id=?",
        vec![primary.clone().into()],
    )
    .await;
    sql(
        &f,
        "UPDATE subscription_changes SET observed_at=?",
        vec![(now(&f) - 8 * 86400).into()],
    )
    .await;
    sql(&f,"INSERT INTO account_history(workspace_id,ad_account_customer_id,collected_at,succeeded_at,error) VALUES (?1,'4829153307',?2,?3,'Collection temporarily unavailable')",vec![primary.clone().into(),now(&f).into(),(now(&f)-40*3600).into()]).await;
    sql(
        &f,
        "INSERT INTO monitoring_runs VALUES (?1,'4829153307',?2,?3,0)",
        vec![
            primary.clone().into(),
            (now(&f) / 86400).into(),
            now(&f).into(),
        ],
    )
    .await;
    seed_assessment(&f).await;
    sql(
        &f,
        "UPDATE subscriptions SET period_start=?1,period_end=?2 WHERE workspace_id=?3",
        vec![
            (now(&f) - 10 * 86400).into(),
            (now(&f) + 20 * 86400).into(),
            primary.clone().into(),
        ],
    )
    .await;
    let mut ids = vec![
        primary.clone(),
        identity::authenticate(&f.ctx.db, f.b.value())
            .await
            .unwrap()
            .unwrap()
            .workspace
            .id()
            .to_owned(),
    ];
    for (n, state) in [
        "active", "active", "active", "past_due", "unpaid", "canceled", "trialing", "active",
        "paused", "canceled",
    ]
    .iter()
    .enumerate()
    {
        ids.push(
            subscription(
                &f,
                &format!("user{:02}@example.com", n + 3),
                state,
                if n == 7 {
                    now(&f) - 86400
                } else {
                    now(&f) + 14 * 86400
                },
                n == 1,
            )
            .await,
        );
    }
    for (n, id) in ids.iter().enumerate() {
        sql(
            &f,
            "UPDATE workspaces SET created_at=?1,sign_up_source='home-primary' WHERE id=?2",
            vec![(now(&f) - (n as i64) * 3 * 86400).into(), id.clone().into()],
        )
        .await;
        sql(
            &f,
            "UPDATE workspace_trials SET started_at=?1 WHERE workspace_id=?2",
            vec![(now(&f) - (n as i64) * 4 * 86400).into(), id.clone().into()],
        )
        .await;
        for offset in 0..30_i64 {
            if (offset + n as i64) % 3 != 0 || n == 4 {
                continue;
            }
            for kind in [
                "viewed",
                "connected",
                "synced",
                "assessed",
                "exported",
                "feedback",
            ] {
                sql(
                    &f,
                    "INSERT INTO workspace_activity VALUES (?1,?2,?3,?4)",
                    vec![
                        id.clone().into(),
                        (now(&f) / 86400 - offset).into(),
                        kind.into(),
                        (n as i64 + 1).into(),
                    ],
                )
                .await;
            }
        }
        if n > 1 {
            let token = identity::sign_in(
                &f.ctx.db,
                &format!("user{:02}@example.com", n + 1),
                &format!("user{:02}@example.com", n + 1),
                "Invented User",
                now(&f),
                None,
            )
            .await
            .unwrap();
            let signed = identity::authenticate(&f.ctx.db, &token)
                .await
                .unwrap()
                .unwrap();
            let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
            let connection = store
                .save_google_connection(
                    "invented",
                    &format!("user{n}@example.com"),
                    "invented-refresh-secret",
                    Settings::load(&f.ctx).unwrap().master_key.as_ref().unwrap(),
                )
                .await
                .unwrap();
            let ad = format!("90000000{n:02}");
            let selection = store
                .create_selection(
                    &connection.id,
                    vec![keysend::workspace::AdAccountChoice {
                        customer_id: ad.clone(),
                        name: format!("Invented studio {n}"),
                        login_customer_id: None,
                    }],
                )
                .await
                .unwrap();
            assert!(store
                .save_selected(&selection, std::slice::from_ref(&ad))
                .await
                .unwrap());
            if n % 3 == 0 {
                sql(&f,"INSERT INTO account_history(workspace_id,ad_account_customer_id,collected_at,succeeded_at) VALUES (?1,?2,?3,?3)",vec![id.clone().into(),ad.clone().into(),(now(&f)-3600).into()]).await;
            }
            if n == 4 {
                store
                    .history_failed(
                        &ad,
                        "Google Ads collection unavailable; reconnect the Google Connection.",
                    )
                    .await
                    .unwrap();
            }
            if n == 5 {
                sql(&f,"INSERT INTO deep_check_failures VALUES (?1,?2,?3,'Review unavailable; retry on the next daily run.')",vec![id.clone().into(),ad.into(),(now(&f)/86400).into()]).await;
            }
        }
    }
    for offset in 0..14_i64 {
        for operation in [
            "sync",
            "deep_check",
            "export",
            "google_request",
            "ai_update",
            "ai_review",
            "ai_research",
            "ai_translation",
            "ai_investigation",
        ] {
            sql(
                &f,
                "INSERT INTO operation_days VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                vec![
                    (now(&f) / 86400 - offset).into(),
                    operation.into(),
                    (if operation.starts_with("ai_") {
                        "invented-model"
                    } else {
                        ""
                    })
                    .into(),
                    (20 + offset).into(),
                    (offset % 4).into(),
                    (20000 + offset * 1000).into(),
                    (30000 + offset * 200).into(),
                    (5000 + offset * 20).into(),
                ],
            )
            .await;
        }
    }
    for offset in (0..10_i64).rev() {
        sql(
            &f,
            "INSERT INTO business_days VALUES (?1,12,12,11,3,?2,2,1,2,?3)",
            vec![
                (now(&f) / 86400 - offset).into(),
                (2 + (9 - offset) / 3).into(),
                ((2 + (9 - offset) / 3) * 4900).into(),
            ],
        )
        .await;
    }
    for (n, id) in ids.iter().enumerate().skip(2) {
        for (offset, state, scheduled) in [
            (8, "active", false),
            (3, if n % 3 == 0 { "past_due" } else { "active" }, true),
            (0, if n % 4 == 0 { "canceled" } else { "active" }, false),
        ] {
            sql(
                &f,
                "INSERT INTO subscription_changes VALUES (?1,?2,?3,?4)",
                vec![
                    id.clone().into(),
                    (now(&f) - offset * 86400).into(),
                    state.into(),
                    scheduled.into(),
                ],
            )
            .await;
        }
    }
    queue(&f).await;
    (f, cookie, primary)
}

#[tokio::test]
#[serial]
#[ignore = "serves an invented admin dashboard for browser verification"]
async fn an_administrator_reads_the_dashboard_in_a_browser() {
    let directory = std::path::PathBuf::from(std::env::var("KEYSEND_BROWSER_CHECK_DIR").unwrap());
    let (f, cookie, id) = invented_dataset().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    std::fs::write(
        directory.join("address"),
        listener.local_addr().unwrap().to_string(),
    )
    .unwrap();
    std::fs::write(directory.join("cookie"), f.a.value()).unwrap();
    std::fs::write(directory.join("admin-cookie"), cookie.value()).unwrap();
    std::fs::write(directory.join("workspace"), id).unwrap();
    let router = f.router.clone();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    while !directory.join("stop").exists() {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    server.abort();
}
