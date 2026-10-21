use migration::{Migrator, MigratorTrait};
use sea_orm::{ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement};

async fn run(db: &DatabaseConnection, sql: &str) {
    db.execute_unprepared(sql).await.unwrap();
}

async fn revert_to(db: &DatabaseConnection, remaining: usize) {
    let steps = u32::try_from(Migrator::migrations().len() - remaining).unwrap();
    Migrator::down(db, Some(steps)).await.unwrap();
}

async fn count(db: &DatabaseConnection, query: &str) -> i64 {
    db.query_one_raw(Statement::from_string(DatabaseBackend::Sqlite, query))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "n")
        .unwrap()
}

async fn tables(db: &DatabaseConnection) -> Vec<String> {
    db.query_all_raw(Statement::from_string(
        DatabaseBackend::Sqlite,
        "SELECT name FROM sqlite_master WHERE type='table' ORDER BY name",
    ))
    .await
    .unwrap()
    .into_iter()
    .map(|row| row.try_get("", "name").unwrap())
    .collect()
}

async fn pool(path: &std::path::Path, connections: u32) -> DatabaseConnection {
    let mut options = sea_orm::ConnectOptions::new(format!("sqlite://{}?mode=rwc", path.display()));
    options
        .max_connections(connections)
        .min_connections(connections)
        .sqlx_logging(false);
    Database::connect(options).await.unwrap()
}

fn remove_database(path: &std::path::Path) {
    for suffix in ["", "-wal", "-shm"] {
        std::fs::remove_file(format!("{}{suffix}", path.display())).ok();
    }
}

#[tokio::test]
async fn dropping_the_report_tables_keeps_the_monitoring_data() {
    for connections in [1, 10, 10, 10] {
        dropping_the_report_tables(connections).await;
    }
}

async fn dropping_the_report_tables(connections: u32) {
    let path = std::env::temp_dir().join(format!(
        "keysend-migration-{}.sqlite",
        keysend::identity::random_token()
    ));
    let db = pool(&path, connections).await;
    Migrator::up(&db, Some(7)).await.unwrap();
    for sql in [
        "INSERT INTO users(id,google_subject_id,email,name) VALUES ('u1','g1','owner@example.com','Owner')",
        "INSERT INTO workspaces(id,user_id) VALUES ('w1','u1')",
        "INSERT INTO users(id,google_subject_id,email,name) VALUES ('u2','g2','legacy@example.com','Legacy')",
        "INSERT INTO workspaces(id,user_id) VALUES ('w2','u2')",
        "DELETE FROM workspace_trials WHERE workspace_id='w2'",
        "INSERT INTO google_connections(id,workspace_id,google_subject_id,email,encrypted_refresh_token) VALUES ('c1','w1','g1','owner@example.com','sealed')",
        "INSERT INTO ad_accounts(id,workspace_id,google_connection_id,customer_id,name) VALUES ('a1','w1','c1','4829153307','Harbour Dental')",
        "INSERT INTO branding(workspace_id,ad_account_customer_id,name,accent_colour) VALUES ('w1','4829153307','Harbour Dental','#5126A6')",
        "INSERT INTO ad_account_profiles(workspace_id,ad_account_customer_id,goal,note) VALUES ('w1','4829153307','leads','Closed on Sundays.')",
        "INSERT INTO ad_account_metadata(workspace_id,ad_account_customer_id,currency,time_zone) VALUES ('w1','4829153307','USD','UTC')",
        "INSERT INTO campaigns(workspace_id,ad_account_customer_id,campaign_id,name,status,channel_type,bidding_strategy) VALUES ('w1','4829153307',1,'Search - Brand','ENABLED','SEARCH','MAXIMIZE_CONVERSIONS')",
        "INSERT INTO campaign_days(workspace_id,ad_account_customer_id,day,campaign_id,device,impressions,clicks,cost_micros,conversions,conversions_value) VALUES ('w1','4829153307',20000,1,1,100,10,5000000,1.0,5.0)",
        "INSERT INTO account_history(workspace_id,ad_account_customer_id,first_day,last_day,collected_at) VALUES ('w1','4829153307',19940,20000,1)",
        "INSERT INTO updates(workspace_id,ad_account_customer_id,day,data) VALUES ('w1','4829153307',20000,'{}')",
        "INSERT INTO subscriptions(workspace_id,stripe_stripe_placeholder) VALUES ('w1','x')",
    ] {
        if sql.contains("stripe_stripe_placeholder") {
            run(&db, "INSERT INTO subscriptions(workspace_id,stripe_subscription_id,created_at,status,period_start,period_end) VALUES ('w1','sub_1',1,'active',1,2)").await;
        } else {
            run(&db, sql).await;
        }
    }
    for sql in [
        "INSERT INTO reports(id,workspace_id,ad_account_customer_id,reporting_start,reporting_end,comparison_start,comparison_end,created_at) VALUES ('r1','w1','4829153307','2026-09-01','2026-09-30','2026-08-01','2026-08-31',1)",
        "INSERT INTO metrics_snapshots(workspace_id,report_id,metrics) VALUES ('w1','r1','{}')",
        "INSERT INTO commentary(workspace_id,report_id,slide,text) VALUES ('w1','r1',1,'Text')",
        "INSERT INTO recommendations(workspace_id,report_id,id,position,action,reason,evidence,origin) VALUES ('w1','r1','x',1,'a','b','c','ai')",
        "INSERT INTO report_feedback(workspace_id,report_id,target,key,text,reason,created_at) VALUES ('w1','r1','finding','k','t','r',1)",
        "INSERT INTO billing_checkouts(workspace_id,id,report_id,kind,amount,created_at) VALUES ('w1','cs_report','r1','Report',499,1)",
        "INSERT INTO billing_checkouts(workspace_id,id,report_id,kind,amount,created_at,paid_at) VALUES ('w1','cs_subscription',NULL,'Subscription',3999,2,3)",
        "INSERT INTO report_unlocks(workspace_id,report_id,checkout_id,paid_at) VALUES ('w1','r1','cs_report',1)",
        "INSERT INTO report_schedules(workspace_id,ad_account_customer_id,comparison) VALUES ('w1','4829153307','previous')",
        "INSERT INTO campaign_exclusions(workspace_id,ad_account_customer_id,campaign_id) VALUES ('w1','4829153307',1)",
    ] {
        run(&db, sql).await;
    }
    assert_eq!(count(&db, "SELECT COUNT(*) AS n FROM reports").await, 1);

    Migrator::up(&db, None).await.unwrap();

    let remaining = tables(&db).await;
    for dropped in [
        "reports",
        "metrics_snapshots",
        "commentary",
        "recommendations",
        "report_feedback",
        "report_unlocks",
        "report_schedules",
        "campaign_exclusions",
    ] {
        assert!(!remaining.contains(&dropped.to_owned()), "{dropped}");
    }
    for kept in [
        "ad_accounts",
        "branding",
        "ad_account_profiles",
        "ad_account_metadata",
        "campaigns",
        "campaign_days",
        "account_history",
        "updates",
        "subscriptions",
        "billing_checkouts",
        "workspace_trials",
    ] {
        assert!(remaining.contains(&kept.to_owned()), "{kept}");
    }
    for query in [
        "SELECT COUNT(*) AS n FROM ad_accounts WHERE name='Harbour Dental'",
        "SELECT COUNT(*) AS n FROM branding WHERE accent_colour='#5126A6'",
        "SELECT COUNT(*) AS n FROM ad_account_profiles WHERE note='Closed on Sundays.'",
        "SELECT COUNT(*) AS n FROM campaign_days WHERE cost_micros=5000000",
        "SELECT COUNT(*) AS n FROM updates WHERE day=20000",
        "SELECT COUNT(*) AS n FROM subscriptions WHERE status='active'",
        "SELECT COUNT(*) AS n FROM billing_checkouts WHERE id='cs_subscription' AND amount=3999 AND paid_at=3",
    ] {
        assert_eq!(count(&db, query).await, 1, "{query}");
    }
    assert_eq!(
        count(&db, "SELECT COUNT(*) AS n FROM billing_checkouts").await,
        1
    );
    let columns = db
        .query_all_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            "SELECT name FROM pragma_table_info('billing_checkouts')",
        ))
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.try_get::<String>("", "name").unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        columns,
        ["workspace_id", "id", "amount", "created_at", "paid_at"]
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) AS n FROM workspace_trials WHERE workspace_id='w2' AND started=0"
        )
        .await,
        1
    );
    drop(db);
    remove_database(&path);
}

#[tokio::test]
async fn a_fresh_database_migrates_through_a_pool_of_ten_connections_every_time() {
    for _ in 0..5 {
        let path = std::env::temp_dir().join(format!(
            "keysend-migration-pool-{}.sqlite",
            keysend::identity::random_token()
        ));
        let db = pool(&path, 10).await;
        Migrator::up(&db, None).await.unwrap();
        assert_eq!(
            count(&db, "SELECT COUNT(*) AS n FROM seaql_migrations").await,
            i64::try_from(Migrator::migrations().len()).unwrap()
        );
        Migrator::up(&db, None).await.unwrap();
        drop(db);
        remove_database(&path);
    }
}

#[tokio::test]
async fn the_breakdown_tables_have_no_rowid_and_go_with_their_ad_account() {
    let path = std::env::temp_dir().join(format!(
        "keysend-migration-{}.sqlite",
        keysend::identity::random_token()
    ));
    let db = Database::connect(format!("sqlite://{}?mode=rwc", path.display()))
        .await
        .unwrap();
    Migrator::up(&db, None).await.unwrap();
    run(&db, "PRAGMA foreign_keys=ON").await;
    let new = [
        "breakdown_history",
        "campaign_network_days",
        "campaign_search_share_days",
        "conversion_action_days",
        "conversion_lag_totals",
        "demographic_totals",
        "hour_totals",
        "keyword_quality",
        "location_totals",
    ];
    for table in new {
        assert_eq!(
            count(&db, &format!("SELECT COUNT(*) AS n FROM sqlite_master WHERE name='{table}' AND sql LIKE '%WITHOUT ROWID' AND sql LIKE '%REFERENCES ad_accounts(workspace_id,customer_id) ON DELETE CASCADE%'")).await,
            1,
            "{table}"
        );
    }
    for sql in [
        "INSERT INTO users(id,google_subject_id,email,name) VALUES ('u1','g1','owner@example.com','Owner')",
        "INSERT INTO workspaces(id,user_id) VALUES ('w1','u1')",
        "INSERT INTO google_connections(id,workspace_id,google_subject_id,email,encrypted_refresh_token) VALUES ('c1','w1','g1','owner@example.com','sealed')",
        "INSERT INTO ad_accounts(id,workspace_id,google_connection_id,customer_id,name) VALUES ('a1','w1','c1','4829153307','Harbour Dental')",
        "INSERT INTO breakdown_history(workspace_id,ad_account_customer_id,kind,first_day,last_day) VALUES ('w1','4829153307','networks',20000,20000)",
        "INSERT INTO campaign_search_share_days(workspace_id,ad_account_customer_id,day,campaign_id,impressions,shown,shown_under,lost_budget,lost_budget_over,lost_rank,lost_rank_over) VALUES ('w1','4829153307',20000,1,10,NULL,1,0.2,0,NULL,0)",
        "INSERT INTO conversion_action_days(workspace_id,ad_account_customer_id,day,campaign_id,action,conversions,conversions_value) VALUES ('w1','4829153307',20000,1,'Booking form',2.0,0.0)",
        "INSERT INTO campaign_network_days(workspace_id,ad_account_customer_id,day,campaign_id,network,impressions,clicks,cost_micros,conversions,conversions_value) VALUES ('w1','4829153307',20000,1,'SEARCH',100,10,5000000,1.0,5.0)",
        "INSERT INTO hour_totals(workspace_id,ad_account_customer_id,day,weekday,hour,impressions,clicks,cost_micros,conversions,conversions_value) VALUES ('w1','4829153307',20000,0,9,100,10,5000000,1.0,5.0)",
        "INSERT INTO location_totals(workspace_id,ad_account_customer_id,day,location_id,name,impressions,clicks,cost_micros,conversions,conversions_value) VALUES ('w1','4829153307',20000,1014044,'Harbour City',100,10,5000000,1.0,5.0)",
        "INSERT INTO keyword_quality(workspace_id,ad_account_customer_id,day,campaign_id,ad_group_id,criterion_id,keyword,match_type,cost_micros,quality_score,ad_relevance,landing_page,expected_ctr) VALUES ('w1','4829153307',20000,1,2,3,'emergency dentist','EXACT',5000000,7,'AVERAGE','ABOVE_AVERAGE','BELOW_AVERAGE')",
        "INSERT INTO demographic_totals(workspace_id,ad_account_customer_id,day,kind,value,impressions,clicks,cost_micros,conversions,conversions_value) VALUES ('w1','4829153307',20000,'age','AGE_RANGE_25_34',100,10,5000000,1.0,5.0)",
        "INSERT INTO conversion_lag_totals(workspace_id,ad_account_customer_id,day,bucket,conversions) VALUES ('w1','4829153307',20000,'LESS_THAN_ONE_DAY',4.0)",
    ] {
        run(&db, sql).await;
    }
    for refused in [
        "INSERT INTO campaign_search_share_days(workspace_id,ad_account_customer_id,day,campaign_id,impressions,shown,shown_under,lost_budget,lost_budget_over,lost_rank,lost_rank_over) VALUES ('w1','4829153307',20001,1,10,0.0999,1,NULL,0,NULL,0)",
        "INSERT INTO keyword_quality(workspace_id,ad_account_customer_id,day,campaign_id,ad_group_id,criterion_id,keyword,match_type,cost_micros,quality_score) VALUES ('w1','4829153307',20001,1,2,3,'x','EXACT',1,11)",
        "INSERT INTO hour_totals(workspace_id,ad_account_customer_id,day,weekday,hour,impressions,clicks,cost_micros,conversions,conversions_value) VALUES ('w1','4829153307',20000,7,9,1,1,1,0,0)",
        "INSERT INTO campaign_network_days(workspace_id,ad_account_customer_id,day,campaign_id,network,impressions,clicks,cost_micros,conversions,conversions_value) VALUES ('w1','0000000000',20000,1,'SEARCH',1,1,1,0,0)",
    ] {
        assert!(db.execute_unprepared(refused).await.is_err(), "{refused}");
    }
    run(
        &db,
        "DELETE FROM ad_accounts WHERE customer_id='4829153307'",
    )
    .await;
    for table in new {
        assert_eq!(
            count(&db, &format!("SELECT COUNT(*) AS n FROM {table}")).await,
            0,
            "{table}"
        );
    }
    revert_to(&db, 10).await;
    let remaining = tables(&db).await;
    for table in new {
        assert!(!remaining.contains(&table.to_owned()), "{table}");
    }
    assert!(remaining.contains(&"campaign_days".to_owned()));
    drop(db);
    std::fs::remove_file(path).ok();
}

#[tokio::test]
async fn the_campaign_settings_table_has_no_rowid_keeps_unset_values_null_and_goes_with_its_ad_account(
) {
    let path = std::env::temp_dir().join(format!(
        "keysend-migration-{}.sqlite",
        keysend::identity::random_token()
    ));
    let db = Database::connect(format!("sqlite://{}?mode=rwc", path.display()))
        .await
        .unwrap();
    Migrator::up(&db, None).await.unwrap();
    run(&db, "PRAGMA foreign_keys=ON").await;
    assert_eq!(
        count(&db, "SELECT COUNT(*) AS n FROM sqlite_master WHERE name='campaign_settings' AND sql LIKE '%WITHOUT ROWID' AND sql LIKE '%PRIMARY KEY(workspace_id,ad_account_customer_id,campaign_id)%' AND sql LIKE '%REFERENCES ad_accounts(workspace_id,customer_id) ON DELETE CASCADE%'").await,
        1
    );
    for sql in [
        "INSERT INTO users(id,google_subject_id,email,name) VALUES ('u1','g1','owner@example.com','Owner')",
        "INSERT INTO workspaces(id,user_id) VALUES ('w1','u1')",
        "INSERT INTO google_connections(id,workspace_id,google_subject_id,email,encrypted_refresh_token) VALUES ('c1','w1','g1','owner@example.com','sealed')",
        "INSERT INTO ad_accounts(id,workspace_id,google_connection_id,customer_id,name) VALUES ('a1','w1','c1','4829153307','Harbour Dental')",
        "INSERT INTO campaign_settings(workspace_id,ad_account_customer_id,campaign_id) VALUES ('w1','4829153307',701)",
        "INSERT INTO campaign_settings(workspace_id,ad_account_customer_id,campaign_id,target_cost_micros,note,alerts,judge_by_conversions) VALUES ('w1','4829153307',702,45000000,'Paused until March.',0,1)",
    ] {
        run(&db, sql).await;
    }
    assert_eq!(
        count(&db, "SELECT COUNT(*) AS n FROM campaign_settings WHERE campaign_id=701 AND target_cost_micros IS NULL AND note IS NULL AND alerts IS NULL AND judge_by_conversions IS NULL").await,
        1
    );
    for refused in [
        "INSERT INTO campaign_settings(workspace_id,ad_account_customer_id,campaign_id) VALUES ('w1','4829153307',701)",
        "INSERT INTO campaign_settings(workspace_id,ad_account_customer_id,campaign_id) VALUES ('w1','0000000000',703)",
        "INSERT INTO campaign_settings(workspace_id,ad_account_customer_id,campaign_id,target_cost_micros) VALUES ('w1','4829153307',703,0)",
        "INSERT INTO campaign_settings(workspace_id,ad_account_customer_id,campaign_id,note) VALUES ('w1','4829153307',703,'')",
        "INSERT INTO campaign_settings(workspace_id,ad_account_customer_id,campaign_id,alerts) VALUES ('w1','4829153307',703,2)",
        "INSERT INTO campaign_settings(workspace_id,ad_account_customer_id,campaign_id,judge_by_conversions) VALUES ('w1','4829153307',703,2)",
    ] {
        assert!(db.execute_unprepared(refused).await.is_err(), "{refused}");
    }
    run(
        &db,
        "DELETE FROM ad_accounts WHERE customer_id='4829153307'",
    )
    .await;
    assert_eq!(
        count(&db, "SELECT COUNT(*) AS n FROM campaign_settings").await,
        0
    );
    revert_to(&db, 11).await;
    assert!(!tables(&db).await.contains(&"campaign_settings".to_owned()));
    drop(db);
    std::fs::remove_file(path).ok();
}

#[tokio::test]
async fn users_keep_english_and_the_language_column_refuses_other_codes() {
    let path = std::env::temp_dir().join(format!(
        "keysend-migration-{}.sqlite",
        keysend::identity::random_token()
    ));
    let db = pool(&path, 1).await;
    Migrator::up(&db, Some(12)).await.unwrap();
    run(
        &db,
        "INSERT INTO users(id,google_subject_id,email,name) VALUES ('u1','g1','owner@example.com','Owner')",
    )
    .await;
    Migrator::up(&db, None).await.unwrap();
    assert_eq!(
        count(&db, "SELECT COUNT(*) AS n FROM users WHERE language='en'").await,
        1
    );
    run(
        &db,
        "INSERT INTO users(id,google_subject_id,email,name,language) VALUES ('u2','g2','pt@example.com','Pt','pt')",
    )
    .await;
    for refused in [
        "INSERT INTO users(id,google_subject_id,email,name,language) VALUES ('u3','g3','x@example.com','X','xx')",
        "INSERT INTO users(id,google_subject_id,email,name,language) VALUES ('u3','g3','x@example.com','X',NULL)",
        "UPDATE users SET language='EN' WHERE id='u1'",
    ] {
        assert!(db.execute_unprepared(refused).await.is_err(), "{refused}");
    }
    revert_to(&db, 12).await;
    assert_eq!(count(&db, "SELECT COUNT(*) AS n FROM users").await, 2);
    drop(db);
    remove_database(&path);
}

#[tokio::test]
async fn translations_have_no_rowid_refuse_english_and_go_with_their_ad_account() {
    let path = std::env::temp_dir().join(format!(
        "keysend-migration-{}.sqlite",
        keysend::identity::random_token()
    ));
    let db = pool(&path, 1).await;
    Migrator::up(&db, None).await.unwrap();
    assert_eq!(
        count(&db, "SELECT COUNT(*) AS n FROM sqlite_master WHERE name='translations' AND sql LIKE '%WITHOUT ROWID' AND sql LIKE '%REFERENCES ad_accounts(workspace_id,customer_id) ON DELETE CASCADE%'").await,
        1
    );
    let hash = "a".repeat(64);
    for sql in [
        "INSERT INTO users(id,google_subject_id,email,name) VALUES ('u1','g1','owner@example.com','Owner')".to_owned(),
        "INSERT INTO workspaces(id,user_id) VALUES ('w1','u1')".to_owned(),
        "INSERT INTO google_connections(id,workspace_id,google_subject_id,email,encrypted_refresh_token) VALUES ('c1','w1','g1','owner@example.com','sealed')".to_owned(),
        "INSERT INTO ad_accounts(id,workspace_id,google_connection_id,customer_id,name) VALUES ('a1','w1','c1','4829153307','Harbour Dental')".to_owned(),
        format!("INSERT INTO translations(workspace_id,ad_account_customer_id,language,source_hash,text,created_at) VALUES ('w1','4829153307','ro','{hash}','Text',1)"),
    ] {
        run(&db, &sql).await;
    }
    for refused in [
        format!("INSERT INTO translations(workspace_id,ad_account_customer_id,language,source_hash,text,created_at) VALUES ('w1','4829153307','ro','{hash}','Again',2)"),
        format!("INSERT INTO translations(workspace_id,ad_account_customer_id,language,source_hash,text,created_at) VALUES ('w1','4829153307','en','{hash}','English',2)"),
        "INSERT INTO translations(workspace_id,ad_account_customer_id,language,source_hash,text,created_at) VALUES ('w1','4829153307','fr','short','Text',2)".to_owned(),
        format!("INSERT INTO translations(workspace_id,ad_account_customer_id,language,source_hash,text,created_at) VALUES ('w1','0000000000','fr','{hash}','Text',2)"),
    ] {
        assert!(db.execute_unprepared(&refused).await.is_err(), "{refused}");
    }
    run(
        &db,
        "DELETE FROM ad_accounts WHERE customer_id='4829153307'",
    )
    .await;
    assert_eq!(
        count(&db, "SELECT COUNT(*) AS n FROM translations").await,
        0
    );
    revert_to(&db, 13).await;
    assert!(!tables(&db).await.contains(&"translations".to_owned()));
    drop(db);
    remove_database(&path);
}

#[tokio::test]
async fn the_sign_up_source_is_optional_short_and_goes_with_its_migration() {
    let path = std::env::temp_dir().join(format!(
        "keysend-migration-{}.sqlite",
        keysend::identity::random_token()
    ));
    let db = pool(&path, 1).await;
    Migrator::up(&db, Some(14)).await.unwrap();
    for sql in [
        "INSERT INTO users(id,google_subject_id,email,name) VALUES ('u1','g1','owner@example.com','Owner')",
        "INSERT INTO workspaces(id,user_id) VALUES ('w1','u1')",
    ] {
        db.execute_unprepared(sql).await.unwrap();
    }
    Migrator::up(&db, None).await.unwrap();
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) AS n FROM workspaces WHERE id='w1' AND sign_up_source IS NULL"
        )
        .await,
        1
    );
    for sql in [
        "UPDATE workspaces SET sign_up_source='home-hero' WHERE id='w1'",
        "INSERT INTO oauth_states(state_hash,browser_hash,pkce_verifier,expires_at,sign_up_source) VALUES ('s1','b1','v1',1,'resource')",
        "INSERT INTO oauth_states(state_hash,browser_hash,pkce_verifier,expires_at) VALUES ('s2','b2','v2',1)",
    ] {
        db.execute_unprepared(sql).await.unwrap();
    }
    let long = "a".repeat(41);
    for refused in [
        "UPDATE workspaces SET sign_up_source='' WHERE id='w1'".to_owned(),
        format!("UPDATE workspaces SET sign_up_source='{long}' WHERE id='w1'"),
        format!("UPDATE oauth_states SET sign_up_source='{long}' WHERE state_hash='s1'"),
    ] {
        assert!(db.execute_unprepared(&refused).await.is_err(), "{refused}");
    }
    revert_to(&db, 14).await;
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) AS n FROM sqlite_master WHERE sql LIKE '%sign_up_source%'"
        )
        .await,
        0
    );
    assert_eq!(count(&db, "SELECT COUNT(*) AS n FROM workspaces").await, 1);
    assert_eq!(
        count(&db, "SELECT COUNT(*) AS n FROM oauth_states").await,
        2
    );
    drop(db);
    remove_database(&path);
}

#[tokio::test]
async fn stored_updates_lose_the_results_sentence_and_keep_the_rest_of_their_summary() {
    let path = std::env::temp_dir().join(format!(
        "keysend-migration-{}.sqlite",
        keysend::identity::random_token()
    ));
    let db = pool(&path, 1).await;
    Migrator::up(&db, Some(15)).await.unwrap();
    for sql in [
        "INSERT INTO users(id,google_subject_id,email,name) VALUES ('u1','g1','owner@example.com','Owner')",
        "INSERT INTO workspaces(id,user_id) VALUES ('w1','u1')",
        "INSERT INTO google_connections(id,workspace_id,google_subject_id,email,encrypted_refresh_token) VALUES ('c1','w1','g1','owner@example.com','sealed')",
        "INSERT INTO ad_accounts(id,workspace_id,google_connection_id,customer_id,name) VALUES ('a1','w1','c1','4829153307','Harbour Dental')",
        r#"INSERT INTO updates(workspace_id,ad_account_customer_id,day,data) VALUES ('w1','4829153307',20000,'{"day":20000,"summary":"Spend rose 2.8% to RON 251. Results cannot be judged.","lines":[]}')"#,
        r#"INSERT INTO updates(workspace_id,ad_account_customer_id,day,data) VALUES ('w1','4829153307',20001,'{"day":20001,"summary":"Conversions rose from 88 to 104.","lines":[]}')"#,
    ] {
        db.execute_unprepared(sql).await.unwrap();
    }
    Migrator::up(&db, None).await.unwrap();
    let summaries: Vec<String> = db
        .query_all_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            "SELECT json_extract(data,'$.summary') AS summary FROM updates ORDER BY day",
        ))
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.try_get("", "summary").unwrap())
        .collect();
    assert_eq!(
        summaries,
        [
            "Spend rose 2.8% to RON 251.",
            "Conversions rose from 88 to 104."
        ]
    );
    drop(db);
    remove_database(&path);
}

#[tokio::test]
async fn the_economics_of_an_ad_account_profile_are_optional_and_refuse_impossible_values() {
    let path = std::env::temp_dir().join(format!(
        "keysend-migration-{}.sqlite",
        keysend::identity::random_token()
    ));
    let db = pool(&path, 1).await;
    Migrator::up(&db, Some(16)).await.unwrap();
    for sql in [
        "INSERT INTO users(id,google_subject_id,email,name) VALUES ('u1','g1','owner@example.com','Owner')",
        "INSERT INTO workspaces(id,user_id) VALUES ('w1','u1')",
        "INSERT INTO google_connections(id,workspace_id,google_subject_id,email,encrypted_refresh_token) VALUES ('c1','w1','g1','owner@example.com','sealed')",
        "INSERT INTO ad_accounts(id,workspace_id,google_connection_id,customer_id,name) VALUES ('a1','w1','c1','4829153307','Harbour Dental')",
        "INSERT INTO ad_account_profiles(workspace_id,ad_account_customer_id,goal,note) VALUES ('w1','4829153307','sales','Garden tools.')",
    ] {
        db.execute_unprepared(sql).await.unwrap();
    }
    Migrator::up(&db, None).await.unwrap();
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) AS n FROM ad_account_profiles WHERE note='Garden tools.' AND margin_basis_points IS NULL AND customer_value_micros IS NULL AND close_rate_basis_points IS NULL AND target_return_hundredths IS NULL"
        )
        .await,
        1
    );
    db.execute_unprepared("UPDATE ad_account_profiles SET margin_basis_points=2500,customer_value_micros=1500000000,close_rate_basis_points=2000,target_return_hundredths=400")
        .await
        .unwrap();
    for refused in [
        "UPDATE ad_account_profiles SET margin_basis_points=0",
        "UPDATE ad_account_profiles SET close_rate_basis_points=10001",
        "UPDATE ad_account_profiles SET customer_value_micros=-1",
        "UPDATE ad_account_profiles SET target_return_hundredths=0",
    ] {
        assert!(db.execute_unprepared(refused).await.is_err(), "{refused}");
    }
    Migrator::down(&db, Some(2)).await.unwrap();
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) AS n FROM ad_account_profiles WHERE note='Garden tools.'"
        )
        .await,
        1
    );
    drop(db);
    remove_database(&path);
}

#[tokio::test]
async fn administrators_have_separate_challenges_sessions_and_throttles() {
    let path = std::env::temp_dir().join(format!(
        "keysend-admin-migration-{}.sqlite",
        keysend::identity::random_token()
    ));
    let db = pool(&path, 1).await;
    Migrator::up(&db, Some(17)).await.unwrap();
    Migrator::up(&db, None).await.unwrap();
    for sql in [
        "INSERT INTO admin_challenges(token_hash,email,code_mac,expires_at) VALUES ('challenge','admin@example.com','mac',600)",
        "INSERT INTO admin_sessions(token_hash,email,created_at,expires_at) VALUES ('session','admin@example.com',0,14400)",
        "INSERT INTO admin_throttle(key,window_start,count) VALUES ('all',0,1)",
    ] { run(&db, sql).await; }
    assert_eq!(
        count(&db, "SELECT attempts AS n FROM admin_challenges").await,
        0
    );
    assert!(db.execute_unprepared("INSERT INTO admin_sessions(token_hash,email,created_at,expires_at) VALUES ('session','admin@example.com',0,14400)").await.is_err());
    revert_to(&db, 17).await;
    for table in ["admin_challenges", "admin_sessions", "admin_throttle"] {
        assert!(!tables(&db).await.contains(&table.to_owned()));
    }
    drop(db);
    remove_database(&path);
}

#[tokio::test]
async fn measurement_keeps_existing_registration_and_collection_dates_unknown() {
    let db = Database::connect("sqlite::memory:").await.unwrap();
    Migrator::up(&db, Some(17)).await.unwrap();
    run(&db, "INSERT INTO users(id,google_subject_id,email,name) VALUES ('u','g','user@example.com','User')").await;
    run(&db, "INSERT INTO workspaces(id,user_id) VALUES ('w','u')").await;
    Migrator::up(&db, None).await.unwrap();
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) AS n FROM workspaces WHERE created_at IS NULL"
        )
        .await,
        1
    );
    for kind in keysend::workspace::measurement::Activity::ALL.map(|kind| kind.as_str()) {
        run(
            &db,
            &format!("INSERT INTO workspace_activity VALUES ('w',20000,'{kind}',1)"),
        )
        .await;
    }
    for op in keysend::workspace::measurement::Operation::ALL {
        run(
            &db,
            &format!(
                "INSERT INTO operation_days VALUES (20000,'{}','',1,0,0,NULL,NULL)",
                op.as_str()
            ),
        )
        .await;
    }
    assert!(db
        .execute_unprepared(
            "INSERT INTO operation_days VALUES (20000,'unknown','',1,0,0,NULL,NULL)"
        )
        .await
        .is_err());
    assert!(db
        .execute_unprepared("INSERT INTO workspace_activity VALUES ('w',20000,'unknown',1)")
        .await
        .is_err());
}

#[tokio::test]
async fn investigations_keep_their_steps_until_the_workspace_or_parent_is_removed() {
    let db = Database::connect("sqlite::memory:").await.unwrap();
    Migrator::up(&db, Some(19)).await.unwrap();
    for sql in [
        "INSERT INTO users(id,google_subject_id,email,name) VALUES ('u','g','user@example.com','User')",
        "INSERT INTO workspaces(id,user_id) VALUES ('w','u')",
        "INSERT INTO operation_days VALUES (20000,'ai_update','model',2,1,456,123,78)",
    ] { run(&db, sql).await; }
    Migrator::up(&db, None).await.unwrap();
    assert_eq!(count(&db, "SELECT COUNT(*) AS n FROM operation_days WHERE operation='ai_update' AND succeeded=2 AND failed=1 AND elapsed_ms=456 AND input_tokens=123 AND output_tokens=78").await, 1);
    for sql in [
        "INSERT INTO operation_days VALUES (20000,'ai_investigation','model',1,0,12,NULL,NULL)",
        "INSERT INTO investigations(id,workspace_id,question,accounts,period,state,created_at) VALUES (1,'w','Where did spend change?','[\"4829153307\"]','last_7_days','created',1)",
        "INSERT INTO investigations(id,workspace_id,follows,question,accounts,period,state,created_at) VALUES (2,'w',1,'Continue','[]','last_30_days','gathering',2)",
        "INSERT INTO investigation_steps VALUES (2,1,1,'{}','answered','[]',12)",
    ] { run(&db, sql).await; }
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) AS n FROM pragma_table_list WHERE name='investigation_steps' AND wr=1"
        )
        .await,
        1
    );
    assert_eq!(count(&db, "SELECT COUNT(*) AS n FROM investigations WHERE model_calls=0 AND input_tokens=0 AND output_tokens=0 AND started_at IS NULL AND finished_at IS NULL AND failure IS NULL AND report IS NULL").await, 2);
    for sql in [
        "UPDATE investigations SET question=printf('%0501d',1)",
        "UPDATE investigations SET period='last_90_days'",
        "UPDATE investigations SET state='running'",
        "UPDATE investigations SET failure='unknown'",
        "UPDATE investigation_steps SET outcome='unknown'",
        "INSERT INTO investigation_steps VALUES (99,1,1,'{}','refused','[]',0)",
        "INSERT INTO investigation_steps VALUES (2,1,2,'{}','repeated','[]',0)",
        "INSERT INTO investigations(workspace_id,question,accounts,period,state,created_at) VALUES ('missing','Question','[]','last_7_days','created',1)",
    ] { assert!(db.execute_unprepared(sql).await.is_err(), "{sql}"); }
    run(
        &db,
        "UPDATE investigations SET question=printf('%0500d',1),failure='interrupted'",
    )
    .await;
    run(&db, "DELETE FROM investigations WHERE id=1").await;
    assert_eq!(
        count(&db, "SELECT COUNT(*) AS n FROM investigations").await,
        0
    );
    assert_eq!(
        count(&db, "SELECT COUNT(*) AS n FROM investigation_steps").await,
        0
    );
    for sql in [
        "INSERT INTO investigations(id,workspace_id,question,accounts,period,state,created_at) VALUES (3,'w','Question','[]','last_7_days','created',1)",
        "INSERT INTO investigation_steps VALUES (3,1,1,'{}','refused','[]',0)",
        "DELETE FROM workspaces WHERE id='w'",
    ] { run(&db, sql).await; }
    assert_eq!(
        count(&db, "SELECT COUNT(*) AS n FROM investigations").await,
        0
    );
    assert_eq!(
        count(&db, "SELECT COUNT(*) AS n FROM investigation_steps").await,
        0
    );
    revert_to(&db, 19).await;
    assert_eq!(count(&db, "SELECT COUNT(*) AS n FROM operation_days WHERE operation='ai_update' AND succeeded=2 AND input_tokens=123").await, 1);
    assert!(db
        .execute_unprepared(
            "INSERT INTO operation_days VALUES (20000,'ai_investigation','model',1,0,0,NULL,NULL)"
        )
        .await
        .is_err());
    Migrator::up(&db, None).await.unwrap();
    assert_eq!(
        keysend::workspace::measurement::Operation::from_schema("agent_investigation")
            .unwrap()
            .as_str(),
        "ai_investigation"
    );
}

#[tokio::test]
async fn optimization_history_is_append_only_and_goes_with_its_ad_account() {
    let db = Database::connect("sqlite::memory:").await.unwrap();
    Migrator::up(&db, None).await.unwrap();
    run(&db, "PRAGMA foreign_keys=ON").await;
    for sql in [
        "INSERT INTO users(id,google_subject_id,email,name) VALUES ('u','g','user@example.com','User')",
        "INSERT INTO workspaces(id,user_id) VALUES ('w','u')",
        "INSERT INTO google_connections(id,workspace_id,google_subject_id,email,encrypted_refresh_token) VALUES ('c','w','g','user@example.com','sealed')",
        "INSERT INTO ad_accounts(id,workspace_id,google_connection_id,customer_id,name) VALUES ('a','w','c','1234567890','Ad Account')",
        "INSERT INTO optimization_policies VALUES ('w','1234567890',1,'{}','active','',1)",
        "INSERT INTO optimization_proposals(id,workspace_id,ad_account_customer_id,key,draft,fingerprint,approval,state,created_at,expires_at) VALUES (1,'w','1234567890','pause_campaign:7','{}','f','required','open',1,2)",
        "INSERT INTO optimization_executions(proposal_id,workspace_id,ad_account_customer_id,day,automatic,started_at) VALUES (1,'w','1234567890',20000,0,1)",
        "INSERT INTO optimization_events(workspace_id,ad_account_customer_id,proposal_id,at,kind,detail) VALUES ('w','1234567890',1,1,'proposed','{}')",
        "INSERT INTO campaign_budgets VALUES ('w','1234567890',7,70,0,1)",
        "INSERT INTO optimization_stops VALUES ('w',1)",
    ] { run(&db, sql).await; }
    for sql in [
        "UPDATE optimization_events SET kind='applied'",
        "UPDATE optimization_policies SET standing='revoked'",
        "UPDATE optimization_proposals SET draft='{\"other\":1}'",
        "UPDATE optimization_proposals SET fingerprint='g'",
        "UPDATE optimization_proposals SET state='unknown'",
        "INSERT INTO optimization_proposals(workspace_id,ad_account_customer_id,key,draft,fingerprint,approval,state,created_at,expires_at) VALUES ('w','1234567890','pause_campaign:7','{}','f','required','open',1,2)",
        "INSERT INTO optimization_policies VALUES ('w','1234567890',0,'{}','active','',1)",
        "INSERT INTO campaign_budgets VALUES ('w','1234567890',8,80,2,1)",
    ] { assert!(db.execute_unprepared(sql).await.is_err(), "{sql}"); }
    run(&db, "UPDATE optimization_proposals SET state='applied'").await;
    run(&db, "INSERT INTO optimization_proposals(workspace_id,ad_account_customer_id,key,draft,fingerprint,approval,state,created_at,expires_at) VALUES ('w','1234567890','pause_campaign:7','{}','f','required','open',1,2)").await;
    run(&db, "DELETE FROM ad_accounts").await;
    for table in [
        "optimization_policies",
        "optimization_proposals",
        "optimization_executions",
        "optimization_events",
        "campaign_budgets",
    ] {
        assert_eq!(
            count(&db, &format!("SELECT COUNT(*) AS n FROM {table}")).await,
            0,
            "{table}"
        );
    }
    run(&db, "DELETE FROM workspaces").await;
    assert_eq!(
        count(&db, "SELECT COUNT(*) AS n FROM optimization_stops").await,
        0
    );
    revert_to(&db, 20).await;
    assert!(!tables(&db)
        .await
        .iter()
        .any(|table| table.starts_with("optimization_")));
    Migrator::up(&db, None).await.unwrap();
}

#[tokio::test]
async fn shadow_evidence_stays_immutable_and_is_removed_with_its_ad_account() {
    let db = Database::connect("sqlite::memory:").await.unwrap();
    Migrator::up(&db, None).await.unwrap();
    run(&db, "PRAGMA foreign_keys=ON").await;
    for sql in [
        "INSERT INTO users(id,google_subject_id,email,name) VALUES ('u','g','user@example.com','User')",
        "INSERT INTO workspaces(id,user_id) VALUES ('w','u')",
        "INSERT INTO google_connections(id,workspace_id,google_subject_id,email,encrypted_refresh_token) VALUES ('c','w','g','user@example.com','sealed')",
        "INSERT INTO ad_accounts(id,workspace_id,google_connection_id,customer_id,name) VALUES ('a','w','c','1234567890','Ad Account')",
        "INSERT INTO optimization_policies VALUES ('w','1234567890',1,'{}','active','',1)",
        "INSERT INTO optimization_shadows VALUES (1,'w','1234567890',1,20000,1,'{}')",
        "INSERT INTO optimization_shadow_events VALUES (1,1,1,'feedback','{}')",
    ] { run(&db, sql).await; }
    for sql in [
        "UPDATE optimization_shadows SET detail='[]'",
        "UPDATE optimization_shadow_events SET detail='[]'",
        "INSERT INTO optimization_shadows VALUES (2,'w','1234567890',1,20000,2,'{}')",
        "INSERT INTO optimization_shadows VALUES (2,'w','1234567890',2,20001,2,'{}')",
    ] {
        assert!(db.execute_unprepared(sql).await.is_err(), "{sql}");
    }
    run(&db, "DELETE FROM ad_accounts").await;
    assert_eq!(
        count(&db, "SELECT COUNT(*) AS n FROM optimization_shadows").await,
        0
    );
    assert_eq!(
        count(&db, "SELECT COUNT(*) AS n FROM optimization_shadow_events").await,
        0
    );
}
