use std::time::Duration;

use chrono::NaiveDate;
use keysend::{
    app::App,
    crypto::MasterKey,
    identity,
    monitoring::{
        audit::{Audit, Counts, Data},
        deep_check::{DeepCheck, Finding},
        findings,
    },
    workspace::{
        history::{Campaign, CampaignDay},
        monitoring::{BreakdownDays, DeepBreakdowns, HourTotal},
        AdAccountChoice, WorkspaceStore,
    },
};
use loco_rs::{
    app::{AppContext, Hooks},
    boot::create_context,
    environment::Environment,
};
use migration::{Migrator, MigratorTrait};
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement, TransactionTrait};
use serial_test::serial;

const KEY: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
const AD: &str = "4829153307";

async fn pooled(connections: u32, idle_milliseconds: u64) -> (AppContext, String) {
    let path =
        std::env::temp_dir().join(format!("keysend-pool-{}.sqlite", identity::random_token()));
    let mut config = App::load_config(&Environment::Test).await.unwrap();
    config.database.uri = format!("sqlite://{}?mode=rwc", path.display());
    config.database.max_connections = connections;
    config.database.min_connections = 0;
    config.database.connect_timeout = 10_000;
    config.database.idle_timeout = idle_milliseconds;
    config.queue = None;
    let ctx = create_context::<App>(&Environment::Test, config)
        .await
        .unwrap();
    Migrator::up(&ctx.db, None).await.unwrap();
    (ctx, path.display().to_string())
}

fn remove(path: &str) {
    for suffix in ["", "-wal", "-shm"] {
        std::fs::remove_file(format!("{path}{suffix}")).ok();
    }
}

async fn pragma(db: &impl ConnectionTrait, name: &str) -> i64 {
    db.query_one_raw(Statement::from_string(
        DatabaseBackend::Sqlite,
        format!("PRAGMA {name}"),
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get_by_index(0)
    .unwrap()
}

#[tokio::test]
#[serial]
async fn every_pooled_connection_gets_the_pragmas_also_after_an_idle_period() {
    let (ctx, path) = pooled(5, 150).await;
    for round in 0..2 {
        let mut held = Vec::new();
        for _ in 0..5 {
            held.push(ctx.db.begin().await.unwrap());
        }
        for (index, connection) in held.iter().enumerate() {
            let at = format!("round {round}, connection {index}");
            assert_eq!(pragma(connection, "synchronous").await, 1, "{at}");
            assert_eq!(pragma(connection, "foreign_keys").await, 1, "{at}");
            assert_eq!(pragma(connection, "busy_timeout").await, 5000, "{at}");
            assert_eq!(pragma(connection, "cache_size").await, -8000, "{at}");
            assert_eq!(pragma(connection, "mmap_size").await, 134_217_728, "{at}");
        }
        drop(held);
        tokio::time::sleep(Duration::from_millis(600)).await;
    }
    drop(ctx);
    remove(&path);
}

async fn signed_in(ctx: &AppContext) -> String {
    let token = identity::sign_in(
        &ctx.db,
        "a",
        "a@example.com",
        "User A",
        chrono::Utc::now().timestamp(),
        None,
    )
    .await
    .unwrap();
    let signed = identity::authenticate(&ctx.db, &token)
        .await
        .unwrap()
        .unwrap();
    let store = WorkspaceStore::new(&ctx.db, &signed.workspace);
    let connection = store
        .save_google_connection(
            "a",
            "a@example.com",
            "never-send-this-refresh-secret",
            &MasterKey::from_base64(KEY).unwrap(),
        )
        .await
        .unwrap();
    let selection = store
        .create_selection(
            &connection.id,
            vec![AdAccountChoice {
                customer_id: AD.into(),
                name: "Northwind Dental".into(),
                login_customer_id: None,
            }],
        )
        .await
        .unwrap();
    assert!(store.save_selected(&selection, &[AD.into()]).await.unwrap());
    token
}

async fn store_of(ctx: &AppContext, token: &str) -> identity::SignedIn {
    identity::authenticate(&ctx.db, token)
        .await
        .unwrap()
        .unwrap()
}

fn deep_check() -> DeepCheck {
    DeepCheck {
        day: 20_000,
        data: Data {
            currency: "EUR".into(),
            reporting: Counts::default(),
            audit: Audit {
                conversion_actions: Some(1),
                ..Default::default()
            },
        },
        by_campaign: Default::default(),
        findings: vec![Finding {
            campaign_id: None,
            rule: findings::Finding {
                key: "tracking".into(),
                title: "Conversion tracking".into(),
                detail: "Conversion tracking has no actions.".into(),
                subjects: vec![],
                spend: 0.0,
            },
        }],
        judge_by_conversions: true,
        recommendations_generated: true,
        assessment: None,
        research: None,
    }
}

fn campaign() -> Campaign {
    Campaign {
        id: 701,
        name: "Search - Real".into(),
        status: "ENABLED".into(),
        channel_type: "SEARCH".into(),
        bidding_strategy: "MAXIMIZE_CONVERSIONS".into(),
        budget_micros: None,
    }
}

async fn with_concurrent_writer<T, F>(
    ctx: &AppContext,
    work: impl FnOnce(AppContext) -> F,
) -> loco_rs::Result<T>
where
    F: std::future::Future<Output = loco_rs::Result<T>> + Send + 'static,
    T: Send + 'static,
{
    let writer = ctx
        .db
        .begin_with_options(sea_orm::TransactionOptions {
            sqlite_transaction_mode: Some(sea_orm::SqliteTransactionMode::Immediate),
            ..Default::default()
        })
        .await
        .unwrap();
    writer
        .execute_unprepared("UPDATE users SET name='Renamed' WHERE google_subject_id='a'")
        .await
        .unwrap();
    let task = tokio::spawn(work(ctx.clone()));
    tokio::time::sleep(Duration::from_millis(300)).await;
    writer.commit().await.unwrap();
    task.await.unwrap()
}

#[tokio::test]
#[serial]
async fn a_transaction_that_reads_first_waits_for_a_concurrent_writer_instead_of_failing() {
    let (ctx, path) = pooled(5, 600_000).await;
    let token = signed_in(&ctx).await;
    let from = NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
    let to = NaiveDate::from_ymd_opt(2026, 10, 2).unwrap();
    let saved = with_concurrent_writer(&ctx, |ctx| {
        let token = token.clone();
        async move {
            let signed = store_of(&ctx, &token).await;
            WorkspaceStore::new(&ctx.db, &signed.workspace)
                .save_history(
                    AD,
                    from,
                    to,
                    &[campaign()],
                    &[CampaignDay {
                        day: from,
                        campaign_id: 701,
                        device: 1,
                        counts: Counts::default(),
                    }],
                    &[],
                )
                .await
        }
    })
    .await;
    saved.expect("save_history waits for the writer");
    let refreshed = with_concurrent_writer(&ctx, |ctx| {
        let token = token.clone();
        async move {
            let signed = store_of(&ctx, &token).await;
            WorkspaceStore::new(&ctx.db, &signed.workspace)
                .refresh_deep_check(AD, &deep_check(), &[])
                .await
        }
    })
    .await;
    refreshed.expect("refresh_deep_check waits for the writer");
    let connected = with_concurrent_writer(&ctx, |ctx| {
        let token = token.clone();
        async move {
            let signed = store_of(&ctx, &token).await;
            WorkspaceStore::new(&ctx.db, &signed.workspace)
                .save_google_connection(
                    "a",
                    "a@example.com",
                    "another-refresh-secret",
                    &MasterKey::from_base64(KEY).unwrap(),
                )
                .await
        }
    })
    .await;
    connected.expect("save_google_connection waits for the writer");
    let breakdown = with_concurrent_writer(&ctx, |ctx| {
        let token = token.clone();
        async move {
            let signed = store_of(&ctx, &token).await;
            WorkspaceStore::new(&ctx.db, &signed.workspace)
                .save_breakdown(AD, 20_000, 20_000, BreakdownDays::Networks(vec![]))
                .await
        }
    })
    .await;
    breakdown.expect("save_breakdown waits for the writer");
    let deep = with_concurrent_writer(&ctx, |ctx| {
        let token = token.clone();
        async move {
            let signed = store_of(&ctx, &token).await;
            WorkspaceStore::new(&ctx.db, &signed.workspace)
                .save_deep_breakdowns(
                    AD,
                    20_000,
                    &DeepBreakdowns {
                        hours: Ok(vec![HourTotal {
                            weekday: 0,
                            hour: 9,
                            counts: Counts::default(),
                        }]),
                        locations: Ok(vec![]),
                        keywords: Ok(vec![]),
                        ages: Ok(vec![]),
                        genders: Ok(vec![]),
                        delay: None,
                    },
                )
                .await
        }
    })
    .await;
    deep.expect("save_deep_breakdowns waits for the writer");
    drop(ctx);
    remove(&path);
}
