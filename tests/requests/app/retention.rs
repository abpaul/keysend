use super::*;
use chrono::{DateTime, NaiveDate};
use keysend::{
    monitoring::{retention, schedule},
    workspace::history::day_number,
};
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement, Value};

const AD: &str = "4829153307";
const NOW: &str = "2026-10-11T12:00:00Z";
const TOKEN: &str = "never-send-this-refresh-secret";

fn at(text: &str) -> i64 {
    DateTime::parse_from_rfc3339(text).unwrap().timestamp()
}

fn utc(text: &str) -> DateTime<chrono::Utc> {
    DateTime::parse_from_rfc3339(text).unwrap().to_utc()
}

fn day(year: i32, month: u32, date: u32) -> i64 {
    day_number(NaiveDate::from_ymd_opt(year, month, date).unwrap())
}

impl Fixture {
    async fn run_sql(&self, query: &str, values: Vec<Value>) {
        self.ctx
            .db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                query,
                values,
            ))
            .await
            .unwrap();
    }

    async fn column(&self, query: &str, values: Vec<Value>) -> Vec<String> {
        self.ctx
            .db
            .query_all_raw(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                query,
                values,
            ))
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.try_get_by_index::<String>(0).unwrap())
            .collect()
    }

    async fn workspace_id(&self) -> String {
        identity::authenticate(&self.ctx.db, self.a.value())
            .await
            .unwrap()
            .unwrap()
            .workspace
            .id()
            .to_owned()
    }

    async fn has_workspace(&self, subject: &str) -> bool {
        !self
            .column(
                "SELECT w.id FROM workspaces w JOIN users u ON u.id=w.user_id WHERE u.google_subject_id=?",
                vec![subject.into()],
            )
            .await
            .is_empty()
    }

    async fn signed_out_long_ago(&self) {
        self.run_sql(
            "UPDATE sessions SET expires_at=0 WHERE user_id=(SELECT id FROM users WHERE google_subject_id='a')",
            vec![],
        )
        .await;
    }

    async fn trial_started(&self, text: &str) {
        let workspace = self.workspace_id().await;
        self.run_sql(
            "UPDATE workspace_trials SET started_at=? WHERE workspace_id=?",
            vec![at(text).into(), workspace.into()],
        )
        .await;
    }

    async fn subscription_ended(&self, period_end: &str) {
        self.subscribe().await;
        let workspace = self.workspace_id().await;
        self.run_sql(
            "UPDATE subscriptions SET status='canceled',period_end=? WHERE workspace_id=?",
            vec![at(period_end).into(), workspace.into()],
        )
        .await;
    }

    async fn retention_run(&self) {
        retention::run(&self.ctx).await.unwrap();
    }

    fn deleted_stripe_customers(&self) -> usize {
        self.calls
            .lock()
            .unwrap()
            .stripe
            .requests
            .iter()
            .filter(|(route, _)| route == "DELETE customers/cus_fixture")
            .count()
    }

    fn google_revocations(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .google
            .revocations
            .iter()
            .map(|form| form["token"].clone())
            .collect()
    }

    async fn remembered_trial(&self, subject: &str) -> Vec<String> {
        self.column(
            "SELECT CAST(started_at AS TEXT) FROM trial_history WHERE subject_hash=?",
            vec![identity::subject_hash(subject).into()],
        )
        .await
    }
}

#[tokio::test]
#[serial]
async fn a_workspace_whose_subscription_ended_a_year_and_a_day_ago_is_deleted_after_google_and_stripe_confirm(
) {
    let f = Fixture::new(false).await;
    f.trial_started("2025-01-01T12:00:00Z").await;
    f.subscription_ended("2025-10-10T12:00:00Z").await;
    f.signed_out_long_ago().await;
    f.retention_run().await;
    assert!(!f.has_workspace("a").await);
    assert!(f.has_workspace("b").await);
    assert_eq!(f.google_revocations(), [TOKEN]);
    assert_eq!(f.deleted_stripe_customers(), 1);
    assert_eq!(
        f.remembered_trial("a").await,
        [at("2025-01-01T12:00:00Z").to_string()]
    );
}

#[tokio::test]
#[serial]
async fn a_workspace_whose_subscription_ended_a_year_less_a_day_ago_stays() {
    let f = Fixture::new(false).await;
    f.trial_started("2025-01-01T12:00:00Z").await;
    f.subscription_ended("2025-10-12T12:00:00Z").await;
    f.signed_out_long_ago().await;
    f.retention_run().await;
    assert!(f.has_workspace("a").await);
    assert!(f.google_revocations().is_empty());
    assert_eq!(f.deleted_stripe_customers(), 0);
    assert!(f.remembered_trial("a").await.is_empty());
}

#[tokio::test]
#[serial]
async fn a_workspace_with_an_active_subscription_stays_however_old_its_trial_is() {
    let f = Fixture::new(false).await;
    f.subscribe().await;
    f.trial_started("2020-01-01T00:00:00Z").await;
    f.signed_out_long_ago().await;
    f.retention_run().await;
    assert!(f.has_workspace("a").await);
    assert!(f.google_revocations().is_empty());
    assert_eq!(f.deleted_stripe_customers(), 0);
}

#[tokio::test]
#[serial]
async fn a_workspace_whose_trial_ended_a_year_and_a_day_ago_with_no_subscription_is_deleted() {
    let f = Fixture::new(false).await;
    f.trial_started("2025-09-26T12:00:00Z").await;
    f.signed_out_long_ago().await;
    f.retention_run().await;
    assert!(!f.has_workspace("a").await);
    assert_eq!(f.google_revocations(), [TOKEN]);
    assert_eq!(f.deleted_stripe_customers(), 0);
    assert_eq!(
        f.remembered_trial("a").await,
        [at("2025-09-26T12:00:00Z").to_string()]
    );
}

#[tokio::test]
#[serial]
async fn a_workspace_whose_trial_ended_a_year_less_a_day_ago_with_no_subscription_stays() {
    let f = Fixture::new(false).await;
    f.trial_started("2025-09-28T12:00:00Z").await;
    f.signed_out_long_ago().await;
    f.retention_run().await;
    assert!(f.has_workspace("a").await);
    assert!(f.google_revocations().is_empty());
}

#[tokio::test]
#[serial]
async fn a_workspace_whose_access_ended_over_a_year_ago_stays_while_its_user_is_signed_in() {
    let f = Fixture::new(false).await;
    f.trial_started("2025-01-01T12:00:00Z").await;
    f.retention_run().await;
    assert!(f.has_workspace("a").await);
    assert!(f.google_revocations().is_empty());
    f.signed_out_long_ago().await;
    f.retention_run().await;
    assert!(!f.has_workspace("a").await);
}

#[tokio::test]
#[serial]
async fn when_google_does_not_confirm_the_revoke_the_workspace_stays_and_the_next_run_deletes_it() {
    let f = Fixture::new(false).await;
    f.trial_started("2025-01-01T12:00:00Z").await;
    f.subscription_ended("2025-10-10T12:00:00Z").await;
    f.signed_out_long_ago().await;
    f.calls.lock().unwrap().google.revoke_failure = true;
    f.retention_run().await;
    assert!(f.has_workspace("a").await);
    assert_eq!(f.google_revocations(), [TOKEN]);
    assert_eq!(f.deleted_stripe_customers(), 0);
    assert!(f.remembered_trial("a").await.is_empty());
    f.calls.lock().unwrap().google.revoke_failure = false;
    f.retention_run().await;
    assert!(!f.has_workspace("a").await);
    assert_eq!(f.deleted_stripe_customers(), 1);
    assert_eq!(f.remembered_trial("a").await.len(), 1);
}

#[tokio::test]
#[serial]
async fn when_stripe_does_not_confirm_the_customer_delete_the_workspace_stays_and_the_next_run_deletes_it(
) {
    let f = Fixture::new(false).await;
    f.trial_started("2025-01-01T12:00:00Z").await;
    f.subscription_ended("2025-10-10T12:00:00Z").await;
    f.signed_out_long_ago().await;
    f.calls.lock().unwrap().stripe.fail = true;
    f.retention_run().await;
    assert!(f.has_workspace("a").await);
    assert_eq!(f.deleted_stripe_customers(), 1);
    assert!(f.remembered_trial("a").await.is_empty());
    f.calls.lock().unwrap().stripe.fail = false;
    f.retention_run().await;
    assert!(!f.has_workspace("a").await);
    assert_eq!(f.deleted_stripe_customers(), 2);
    assert_eq!(f.remembered_trial("a").await.len(), 1);
}

#[tokio::test]
#[serial]
async fn a_user_whose_trial_started_over_24_months_ago_and_deleted_the_workspace_gets_a_new_trial()
{
    let f = Fixture::new(false).await;
    f.run_sql(
        "INSERT INTO trial_history(subject_hash,started_at) VALUES(?,?)",
        vec![
            identity::subject_hash("c").into(),
            at("2024-10-10T12:00:00Z").into(),
        ],
    )
    .await;
    f.retention_run().await;
    assert!(f.remembered_trial("c").await.is_empty());
    let token = identity::sign_in(&f.ctx.db, "c", "c@example.com", "User C", at(NOW), None)
        .await
        .unwrap();
    let page = f
        .request
        .get("/subscription")
        .add_cookie(Cookie::new(SESSION_COOKIE, token))
        .await
        .text();
    assert!(page.contains("Your Trial has 14 days left."), "{page}");
}

#[tokio::test]
#[serial]
async fn a_user_whose_trial_started_under_24_months_ago_and_deleted_the_workspace_gets_no_new_trial(
) {
    let f = Fixture::new(false).await;
    f.run_sql(
        "INSERT INTO trial_history(subject_hash,started_at) VALUES(?,?)",
        vec![
            identity::subject_hash("c").into(),
            at("2024-10-12T12:00:00Z").into(),
        ],
    )
    .await;
    f.retention_run().await;
    assert_eq!(
        f.remembered_trial("c").await,
        [at("2024-10-12T12:00:00Z").to_string()]
    );
    let token = identity::sign_in(&f.ctx.db, "c", "c@example.com", "User C", at(NOW), None)
        .await
        .unwrap();
    let page = f
        .request
        .get("/subscription")
        .add_cookie(Cookie::new(SESSION_COOKIE, token))
        .await
        .text();
    assert!(page.contains("Your Trial ended on"), "{page}");
}

const DAILY_ROWS: [(&str, &str); 13] = [
    (
        "campaign_days",
        "INSERT INTO campaign_days(workspace_id,ad_account_customer_id,day,campaign_id,device,impressions,clicks,cost_micros,conversions,conversions_value) VALUES(?1,?2,?3,701,1,1,1,1,0,0)",
    ),
    (
        "campaign_network_days",
        "INSERT INTO campaign_network_days(workspace_id,ad_account_customer_id,day,campaign_id,network,impressions,clicks,cost_micros,conversions,conversions_value) VALUES(?1,?2,?3,701,'SEARCH',1,1,1,0,0)",
    ),
    (
        "campaign_search_share_days",
        "INSERT INTO campaign_search_share_days(workspace_id,ad_account_customer_id,day,campaign_id,impressions,shown,shown_under,lost_budget,lost_budget_over,lost_rank,lost_rank_over) VALUES(?1,?2,?3,701,10,0.5,0,0.1,0,0.1,0)",
    ),
    (
        "conversion_action_days",
        "INSERT INTO conversion_action_days(workspace_id,ad_account_customer_id,day,campaign_id,action,conversions,conversions_value) VALUES(?1,?2,?3,701,'Calls',1,1)",
    ),
    (
        "conversion_lag_totals",
        "INSERT INTO conversion_lag_totals(workspace_id,ad_account_customer_id,day,bucket,conversions) VALUES(?1,?2,?3,'0-1',1)",
    ),
    (
        "demographic_totals",
        "INSERT INTO demographic_totals(workspace_id,ad_account_customer_id,day,kind,value,impressions,clicks,cost_micros,conversions,conversions_value) VALUES(?1,?2,?3,'age','25-34',1,1,1,0,0)",
    ),
    (
        "hour_totals",
        "INSERT INTO hour_totals(workspace_id,ad_account_customer_id,day,weekday,hour,impressions,clicks,cost_micros,conversions,conversions_value) VALUES(?1,?2,?3,1,9,1,1,1,0,0)",
    ),
    (
        "keyword_quality",
        "INSERT INTO keyword_quality(workspace_id,ad_account_customer_id,day,campaign_id,ad_group_id,criterion_id,keyword,match_type,cost_micros) VALUES(?1,?2,?3,701,1,1,'crowns','EXACT',1)",
    ),
    (
        "location_totals",
        "INSERT INTO location_totals(workspace_id,ad_account_customer_id,day,location_id,name,impressions,clicks,cost_micros,conversions,conversions_value) VALUES(?1,?2,?3,1,'Leeds',1,1,1,0,0)",
    ),
    (
        "updates",
        "INSERT INTO updates(workspace_id,ad_account_customer_id,day,data) VALUES(?1,?2,?3,'{}')",
    ),
    (
        "alerts",
        "INSERT INTO alerts(workspace_id,ad_account_customer_id,day,event_key,data,body,created_at) VALUES(?1,?2,?3,'campaign_stopped','{}','body',0)",
    ),
    (
        "alert_evaluations",
        "INSERT INTO alert_evaluations(workspace_id,ad_account_customer_id,day) VALUES(?1,?2,?3)",
    ),
    (
        "monitoring_runs",
        "INSERT INTO monitoring_runs(workspace_id,ad_account_customer_id,day,claimed_at) VALUES(?1,?2,?3,0)",
    ),
];

impl Fixture {
    async fn ad_account_zone(&self, zone: &str) {
        let workspace = self.workspace_id().await;
        self.run_sql(
            "INSERT INTO ad_account_metadata(workspace_id,ad_account_customer_id,currency,time_zone) VALUES(?,?,'EUR',?) ON CONFLICT DO UPDATE SET time_zone=excluded.time_zone",
            vec![workspace.into(), AD.into(), zone.into()],
        )
        .await;
    }

    async fn daily_row(&self, insert: &str, day: i64) {
        let workspace = self.workspace_id().await;
        self.run_sql(insert, vec![workspace.into(), AD.into(), day.into()])
            .await;
    }

    async fn days_kept(&self, table: &str) -> Vec<String> {
        self.column(
            &format!("SELECT CAST(day AS TEXT) FROM {table} ORDER BY day"),
            vec![],
        )
        .await
    }
}

#[tokio::test]
#[serial]
async fn the_daily_numbers_and_updates_of_an_ad_account_older_than_24_months_are_deleted() {
    let f = Fixture::new(false).await;
    f.ad_account_zone("UTC").await;
    let old = day(2024, 10, 10);
    let edge = day(2024, 10, 11);
    for (_, insert) in DAILY_ROWS {
        f.daily_row(insert, old).await;
        f.daily_row(insert, edge).await;
    }
    f.retention_run().await;
    for (table, _) in DAILY_ROWS {
        assert_eq!(f.days_kept(table).await, [edge.to_string()], "{table}");
    }
}

#[tokio::test]
#[serial]
async fn the_age_of_an_ad_account_day_follows_the_time_zone_of_the_ad_account() {
    let f = Fixture::new(false).await;
    f.ad_account_zone("Pacific/Auckland").await;
    f.daily_row(DAILY_ROWS[0].1, day(2024, 10, 11)).await;
    f.daily_row(DAILY_ROWS[0].1, day(2024, 10, 12)).await;
    f.retention_run().await;
    assert_eq!(
        f.days_kept("campaign_days").await,
        [day(2024, 10, 12).to_string()]
    );
}

fn noted(day: i64, text: &str) -> keysend::monitoring::detection::Detection {
    let mut detection = keysend::monitoring::detection::detect(
        &[],
        &[],
        &[],
        day,
        day,
        keysend::monitoring::campaign_settings::AllFollowAccount { judged: true },
        "USD",
    );
    detection.yesterday = vec![text.into()];
    detection
}

#[tokio::test]
#[serial]
async fn the_ad_account_page_of_an_ended_workspace_lists_no_update_older_than_24_months() {
    let f = Fixture::new(false).await;
    f.ad_account_zone("UTC").await;
    f.trial_started("2024-09-01T12:00:00Z").await;
    f.subscription_ended("2026-06-01T12:00:00Z").await;
    for (date, summary) in [
        (day(2024, 10, 10), "Spend fell on the old day."),
        (day(2024, 10, 11), "Spend rose on the edge day."),
        (day(2026, 5, 20), "Spend held on the recent day."),
    ] {
        let workspace = f.workspace_id().await;
        f.run_sql(
            "INSERT INTO updates(workspace_id,ad_account_customer_id,day,data) VALUES(?,?,?,?)",
            vec![
                workspace.into(),
                AD.into(),
                date.into(),
                json!({"summary": summary, "detection": noted(date, summary)})
                    .to_string()
                    .into(),
            ],
        )
        .await;
    }
    let before = f.page("/ad-accounts/4829153307").await;
    assert!(before.contains("Spend fell on the old day."), "{before}");
    f.retention_run().await;
    let after = f.page("/ad-accounts/4829153307").await;
    assert!(!after.contains("Spend fell on the old day."), "{after}");
    assert!(after.contains("Spend rose on the edge day."));
    assert!(after.contains("Spend held on the recent day."));
}

#[tokio::test]
#[serial]
async fn the_account_changes_of_an_ad_account_older_than_24_months_are_deleted() {
    let f = Fixture::new(false).await;
    f.ad_account_zone("UTC").await;
    let workspace = f.workspace_id().await;
    for (changed_at, resource) in [
        ("2024-10-10 23:59:59", "old"),
        ("2024-10-11 00:00:00", "edge"),
    ] {
        f.run_sql(
            "INSERT INTO account_changes(workspace_id,ad_account_customer_id,changed_at,resource_name,resource_type,operation,changed_fields,old_value,new_value) VALUES(?,?,?,?,'CAMPAIGN_BUDGET','UPDATE','','','')",
            vec![
                workspace.clone().into(),
                AD.into(),
                changed_at.into(),
                resource.into(),
            ],
        )
        .await;
    }
    f.retention_run().await;
    assert_eq!(
        f.column("SELECT resource_name FROM account_changes", vec![])
            .await,
        ["edge"]
    );
}

#[tokio::test]
#[serial]
async fn the_findings_and_recommendations_of_an_ad_account_older_than_24_months_are_deleted_and_the_current_ones_stay(
) {
    let f = Fixture::new(false).await;
    f.ad_account_zone("UTC").await;
    let workspace = f.workspace_id().await;
    let first = day(2023, 1, 1);
    for (key, active, last_day) in [
        ("ended-old", 0, day(2024, 10, 10)),
        ("ended-edge", 0, day(2024, 10, 11)),
        ("still-active", 1, day(2024, 1, 1)),
        ("closed-old", 1, day(2026, 10, 10)),
        ("closed-edge", 1, day(2026, 10, 10)),
    ] {
        f.run_sql(
            "INSERT INTO monitoring_findings(workspace_id,ad_account_customer_id,finding_key,data,active,first_day,last_day) VALUES(?,?,?,'{}',?,?,?)",
            vec![
                workspace.clone().into(),
                AD.into(),
                key.into(),
                active.into(),
                first.into(),
                last_day.into(),
            ],
        )
        .await;
    }
    for (key, state, opened, closed) in [
        ("ended-old", "resolved", first, None),
        ("ended-edge", "resolved", first, None),
        ("still-active", "open", first, None),
        ("closed-old", "done", first, Some(day(2024, 10, 10))),
        ("closed-edge", "not_useful", first, Some(day(2024, 10, 11))),
    ] {
        f.run_sql(
            "INSERT INTO monitoring_recommendations(workspace_id,ad_account_customer_id,finding_key,data,state,opened_day,closed_day) VALUES(?,?,?,'{}',?,?,?)",
            vec![
                workspace.clone().into(),
                AD.into(),
                key.into(),
                state.into(),
                opened.into(),
                closed.into(),
            ],
        )
        .await;
    }
    f.retention_run().await;
    assert_eq!(
        f.column(
            "SELECT finding_key FROM monitoring_findings ORDER BY finding_key",
            vec![]
        )
        .await,
        ["closed-edge", "closed-old", "ended-edge", "still-active"]
    );
    assert_eq!(
        f.column(
            "SELECT finding_key FROM monitoring_recommendations ORDER BY finding_key",
            vec![]
        )
        .await,
        ["closed-edge", "ended-edge", "still-active"]
    );
}

#[test]
fn the_retention_job_is_due_in_the_03_utc_hour_only() {
    assert!(retention::due(utc("2026-10-11T03:00:00Z")));
    assert!(retention::due(utc("2026-10-11T03:59:59Z")));
    assert!(!retention::due(utc("2026-10-11T02:59:59Z")));
    assert!(!retention::due(utc("2026-10-11T04:00:00Z")));
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn the_queue_runs_the_retention_job_once_a_day_and_deletes_the_inactive_workspace() {
    let f = Fixture::queued_at("2026-10-11T03:00:00Z").await;
    f.trial_started("2025-09-26T12:00:00Z").await;
    f.signed_out_long_ago().await;
    let queued = || async {
        f.column(
            "SELECT status FROM sqlt_loco_queue WHERE name='RetentionWorker'",
            vec![],
        )
        .await
    };
    schedule::queue_retention(&f.ctx, utc("2026-10-11T04:00:00Z"))
        .await
        .unwrap();
    assert!(queued().await.is_empty());
    schedule::queue_retention(&f.ctx, utc("2026-10-11T03:00:00Z"))
        .await
        .unwrap();
    for _ in 0..40 {
        if !f.has_workspace("a").await && queued().await == ["completed"] {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert!(!f.has_workspace("a").await);
    assert!(f.has_workspace("b").await);
    assert_eq!(queued().await, ["completed"]);
    assert_eq!(f.google_revocations(), [TOKEN]);
}
