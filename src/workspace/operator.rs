use chrono::{DateTime, Utc};
use loco_rs::Result;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, FromQueryResult, Statement, Value,
};
use serde::{Deserialize, Serialize};

use super::{
    history::day_number,
    measurement::{business_values, user_activity_kinds, BUSINESS_FIGURES, BUSINESS_STATE},
};

pub struct OperatorStore<'a> {
    db: &'a DatabaseConnection,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct Figures {
    pub users: i64,
    pub workspaces: i64,
    pub ad_accounts: i64,
    pub trials: i64,
    pub paying: i64,
    pub payment_due: i64,
    pub scheduled_cancellations: i64,
    pub canceled: i64,
    pub mrr_cents: i64,
}
#[derive(Debug, Serialize, FromQueryResult)]
pub struct Engagement {
    pub active7: i64,
    pub active30: i64,
    pub registrations: i64,
    pub unknown_registrations: i64,
    pub trial_ended: i64,
    pub converted: i64,
    pub distinct_ad_accounts: i64,
    pub connected: i64,
    pub synced: i64,
    pub assessed: i64,
    pub exported: i64,
    pub returning_workspaces: i64,
    pub inactive_paying: i64,
}
#[derive(Debug, Serialize, FromQueryResult)]
pub struct Customer {
    pub id: String,
    pub email: String,
    pub created_at: Option<i64>,
    pub source: Option<String>,
    pub status: String,
    pub ad_accounts: i64,
    pub last_activity: Option<i64>,
    pub last_export: Option<i64>,
    pub stage: String,
    pub stale: i64,
    pub failing: i64,
    pub inactive: i64,
}
#[derive(Debug, Serialize, FromQueryResult)]
pub struct AdAccount {
    pub workspace_id: String,
    pub email: String,
    pub customer_id: String,
    pub name: String,
    pub succeeded_at: Option<i64>,
    pub last_run: Option<i64>,
    pub completed: Option<i64>,
    pub deep_day: Option<i64>,
    pub assessment: i64,
    pub history_error: Option<String>,
    pub google_error: Option<String>,
    pub deep_error: Option<String>,
}
#[derive(Debug, Serialize, FromQueryResult)]
pub struct Subscription {
    pub status: String,
    pub period_start: i64,
    pub period_end: i64,
    pub cancel_at_period_end: i64,
    pub canceled_at: Option<i64>,
}
#[derive(Debug, Serialize, FromQueryResult)]
pub struct Trial {
    pub started_at: i64,
    pub ended_at: i64,
    pub started: i64,
}
#[derive(Debug, Serialize, FromQueryResult)]
pub struct Change {
    pub observed_at: i64,
    pub status: String,
    pub cancel_at_period_end: i64,
}
#[derive(Debug, Serialize, FromQueryResult)]
pub struct ActivityCount {
    pub kind: String,
    pub count: i64,
    pub first_day: i64,
    pub last_day: i64,
}
#[derive(Debug, Serialize, FromQueryResult)]
pub struct BusinessPoint {
    pub day: i64,
    pub mrr_cents: i64,
    pub paying: i64,
}
#[derive(Debug, Serialize, FromQueryResult)]
pub struct ActivationPoint {
    pub week: i64,
    pub registrations: i64,
    pub connected: i64,
    pub synced: i64,
    pub assessed: i64,
    pub exported: i64,
}
#[derive(Debug, Serialize, FromQueryResult)]
pub struct Week {
    pub week: i64,
    pub kind: String,
    pub count: i64,
    pub workspaces: i64,
}
#[derive(Debug, Serialize, FromQueryResult)]
pub struct Month {
    pub month: String,
    pub new_subscriptions: i64,
    pub cancellations: i64,
    pub payment_failures: i64,
    pub opening_paying: Option<i64>,
}
#[derive(Debug, Serialize, FromQueryResult)]
pub struct OperationTotal {
    pub operation: String,
    pub succeeded: i64,
    pub failed: i64,
    pub elapsed_ms: i64,
}
#[derive(Debug, Serialize, FromQueryResult)]
pub struct AiDay {
    pub day: i64,
    pub model: String,
    pub calls: i64,
    pub failed: i64,
    pub elapsed_ms: i64,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
}
#[derive(Debug, Serialize, FromQueryResult)]
pub struct Queue {
    pub failed: i64,
    pub processing: i64,
    pub oldest_due: Option<String>,
}
#[derive(Debug, Serialize, FromQueryResult)]
pub struct Failure {
    pub workspace_id: Option<String>,
    pub email: Option<String>,
    pub customer_id: Option<String>,
    pub source: String,
    pub occurred_at: i64,
    pub error: String,
}

#[derive(Debug, Deserialize, Serialize, Default)]
#[serde(default)]
pub struct CustomerQuery {
    pub q: String,
    pub status: String,
    pub sort: String,
    pub page: String,
}
impl CustomerQuery {
    pub fn normalized(mut self) -> Self {
        self.q = self.q.trim().chars().take(320).collect();
        if ![
            "paying",
            "trial",
            "payment_due",
            "scheduled",
            "canceled",
            "ended",
        ]
        .contains(&self.status.as_str())
        {
            self.status.clear();
        }
        if !["created", "activity"].contains(&self.sort.as_str()) {
            self.sort = "created".into();
        }
        self.page = self
            .page
            .parse::<i64>()
            .unwrap_or(1)
            .clamp(1, i64::MAX / 50)
            .to_string();
        self
    }
    pub fn number(&self) -> i64 {
        self.page.parse().unwrap_or(1)
    }
}
fn statement(query: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Sqlite, query, values)
}
impl<'a> OperatorStore<'a> {
    pub fn new(db: &'a DatabaseConnection) -> Self {
        Self { db }
    }
    async fn rows<T: FromQueryResult>(&self, query: &str, values: Vec<Value>) -> Result<Vec<T>> {
        self.db
            .query_all_raw(statement(query, values))
            .await?
            .iter()
            .map(|row| Ok(T::from_query_result(row, "")?))
            .collect()
    }
    pub async fn figures(&self, now: DateTime<Utc>) -> Result<Figures> {
        Ok(self
            .rows(
                &format!("{BUSINESS_STATE} {BUSINESS_FIGURES}"),
                business_values(now),
            )
            .await?
            .remove(0))
    }
    pub async fn recording_started(&self) -> Result<i64> {
        let row=self.db.query_one_raw(statement("SELECT applied_at FROM seaql_migrations WHERE version='m20261020_000019_measurement'",vec![])).await?.expect("measurement migration is applied");
        Ok(row.try_get("", "applied_at")?)
    }
    pub async fn engagement(&self, now: DateTime<Utc>) -> Result<Engagement> {
        let kinds = user_activity_kinds();
        let day = day_number(now.date_naive());
        let mut values = business_values(now);
        values.extend([day.into(), (day - 29).into(), (day - 6).into()]);
        Ok(self.rows(&format!("{BUSINESS_STATE}, activity AS (SELECT * FROM workspace_activity WHERE day BETWEEN ?5 AND ?4), user_activity AS (SELECT * FROM activity WHERE kind IN ({kinds})) SELECT
        (SELECT COUNT(DISTINCT workspace_id) FROM user_activity WHERE day>=?6) active7,
        (SELECT COUNT(DISTINCT workspace_id) FROM user_activity) active30,
        (SELECT COUNT(*) FROM workspaces WHERE created_at BETWEEN ?1-30*86400 AND ?1) registrations,
        (SELECT COUNT(*) FROM workspaces WHERE created_at IS NULL) unknown_registrations,
        (SELECT COUNT(*) FROM workspace_trials WHERE started=1 AND started_at+?2 BETWEEN ?1-90*86400 AND ?1) trial_ended,
        (SELECT COUNT(*) FROM workspace_trials t WHERE started=1 AND started_at+?2 BETWEEN ?1-90*86400 AND ?1 AND EXISTS(SELECT 1 FROM billing_checkouts b WHERE b.workspace_id=t.workspace_id AND paid_at IS NOT NULL AND paid_at<=?1)) converted,
        (SELECT COUNT(DISTINCT customer_id) FROM ad_accounts) distinct_ad_accounts,
        (SELECT COUNT(*) FROM workspaces w WHERE EXISTS(SELECT 1 FROM ad_accounts a WHERE a.workspace_id=w.id)) connected,
        (SELECT COUNT(DISTINCT workspace_id) FROM workspace_activity WHERE kind='synced' AND day<=?4) synced,
        (SELECT COUNT(DISTINCT workspace_id) FROM workspace_activity WHERE kind='assessed' AND day<=?4) assessed,
        (SELECT COUNT(DISTINCT workspace_id) FROM workspace_activity WHERE kind='exported' AND day<=?4) exported,
        (SELECT COUNT(*) FROM (SELECT workspace_id FROM user_activity GROUP BY workspace_id HAVING COUNT(DISTINCT (day+3)/7)>=2)) returning_workspaces,
        (SELECT COUNT(*) FROM state s WHERE paying=1 AND NOT EXISTS(SELECT 1 FROM user_activity a WHERE a.workspace_id=s.id)) inactive_paying"),values).await?.remove(0))
    }
    fn customers_sql() -> String {
        format!("{BUSINESS_STATE}, activity AS (SELECT workspace_id,MAX(CASE WHEN kind IN ({}) THEN day END) last_activity,MAX(CASE WHEN kind='exported' THEN day END) last_export,MAX(CASE kind WHEN 'exported' THEN 4 WHEN 'assessed' THEN 3 WHEN 'synced' THEN 2 WHEN 'connected' THEN 1 ELSE 0 END) stage FROM workspace_activity WHERE day<=?4 GROUP BY workspace_id), accounts AS (SELECT a.workspace_id,COUNT(*) ad_accounts,SUM(h.succeeded_at IS NULL OR h.succeeded_at<?1-36*3600) stale FROM ad_accounts a LEFT JOIN account_history h ON h.workspace_id=a.workspace_id AND h.ad_account_customer_id=a.customer_id GROUP BY a.workspace_id)
        SELECT s.id,u.email,s.created_at,s.sign_up_source source,CASE WHEN scheduled=1 THEN 'scheduled' WHEN paying=1 THEN 'paying' WHEN payment_due=1 THEN 'payment_due' WHEN canceled=1 THEN 'canceled' WHEN trial=1 THEN 'trial' ELSE 'ended' END status,
        COALESCE(a.ad_accounts,0) ad_accounts,ac.last_activity,ac.last_export,
        CASE WHEN ac.stage=4 THEN 'Exported' WHEN ac.stage=3 THEN 'Assessed' WHEN ac.stage=2 THEN 'Synced' WHEN a.ad_accounts>0 THEN 'Connected' ELSE 'Registered' END stage,
        COALESCE(a.stale,0) stale,EXISTS(SELECT 1 FROM deep_check_failures d WHERE d.workspace_id=s.id) failing,
        (paying=1 AND COALESCE(ac.last_activity,0)<?4-29) inactive
        FROM state s JOIN users u ON u.id=s.user_id LEFT JOIN activity ac ON ac.workspace_id=s.id LEFT JOIN accounts a ON a.workspace_id=s.id",user_activity_kinds())
    }
    pub async fn customers(
        &self,
        now: DateTime<Utc>,
        query: &CustomerQuery,
    ) -> Result<Vec<Customer>> {
        let mut values = business_values(now);
        values.push(day_number(now.date_naive()).into());
        let escaped = query
            .q
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        values.extend([
            query.q.clone().into(),
            format!("%{escaped}%").into(),
            ((query.number() - 1) * 50).into(),
        ]);
        let status = match query.status.as_str() {
            "paying" => "s.paying=1",
            "scheduled" => "s.scheduled=1",
            "trial" => "s.trial=1",
            "payment_due" => "s.payment_due=1",
            "canceled" => "s.canceled=1",
            "ended" => "s.paying=0 AND s.trial=0 AND s.payment_due=0 AND s.canceled=0",
            _ => "1=1",
        };
        let order = if query.sort == "activity" {
            "ac.last_activity"
        } else {
            "s.created_at"
        };
        self.rows(&format!("{} WHERE (?5='' OR u.email LIKE ?6 ESCAPE '\\' OR s.id=?5) AND ({status}) ORDER BY {order} DESC,s.id LIMIT 51 OFFSET ?7",Self::customers_sql()),values).await
    }
    pub async fn customer(&self, now: DateTime<Utc>, id: &str) -> Result<Option<Customer>> {
        let mut values = business_values(now);
        values.extend([day_number(now.date_naive()).into(), id.into()]);
        Ok(self
            .rows(
                &format!("{} WHERE s.id=?5 LIMIT 1", Self::customers_sql()),
                values,
            )
            .await?
            .pop())
    }
    pub async fn attention(&self, now: DateTime<Utc>) -> Result<Vec<Customer>> {
        let mut values = business_values(now);
        values.push(day_number(now.date_naive()).into());
        self.rows(&format!("SELECT * FROM ({}) WHERE status IN ('payment_due','scheduled') OR (status IN ('paying','trial') AND (stale>0 OR failing=1)) OR (status='paying' AND inactive=1) ORDER BY stale DESC,email LIMIT 10",Self::customers_sql()),values).await
    }
    pub async fn recent(&self, now: DateTime<Utc>) -> Result<Vec<Customer>> {
        let mut values = business_values(now);
        values.push(day_number(now.date_naive()).into());
        self.rows(&format!("{} WHERE s.created_at IS NOT NULL AND s.created_at<=?1 ORDER BY s.created_at DESC,s.id LIMIT 5",Self::customers_sql()),values).await
    }
    pub async fn subscription(&self, id: &str) -> Result<Option<Subscription>> {
        Ok(self.rows("SELECT status,period_start,period_end,cancel_at_period_end,canceled_at FROM subscriptions WHERE workspace_id=? LIMIT 1",vec![id.into()]).await?.pop())
    }
    pub async fn trial(&self, id: &str) -> Result<Option<Trial>> {
        Ok(self.rows("SELECT started_at,started_at+?2 ended_at,started FROM workspace_trials WHERE workspace_id=?1 LIMIT 1",vec![id.into(),crate::monitoring::access::TRIAL_SECONDS.into()]).await?.pop())
    }
    pub async fn changes(&self, id: &str) -> Result<Vec<Change>> {
        self.rows("SELECT observed_at,status,cancel_at_period_end FROM subscription_changes WHERE workspace_id=? ORDER BY observed_at DESC LIMIT 100",vec![id.into()]).await
    }
    pub async fn activity(
        &self,
        now: DateTime<Utc>,
        id: &str,
        recent: bool,
    ) -> Result<Vec<ActivityCount>> {
        let day = day_number(now.date_naive());
        self.rows("SELECT kind,SUM(count) count,MIN(day) first_day,MAX(day) last_day FROM workspace_activity WHERE workspace_id=?1 AND day BETWEEN ?2 AND ?3 GROUP BY kind LIMIT 6",vec![id.into(),(if recent {day-29} else {0}).into(),day.into()]).await
    }
    pub async fn ad_accounts(
        &self,
        now: DateTime<Utc>,
        id: Option<&str>,
    ) -> Result<Vec<AdAccount>> {
        self.rows("SELECT a.workspace_id,u.email,a.customer_id,a.name,h.succeeded_at,r.day last_run,r.completed,d.day deep_day,CASE WHEN json_type(d.data,'$.assessment')='object' THEN 1 ELSE 0 END assessment,h.error history_error,g.detail google_error,f.error deep_error FROM ad_accounts a JOIN workspaces w ON w.id=a.workspace_id JOIN users u ON u.id=w.user_id LEFT JOIN account_history h ON h.workspace_id=a.workspace_id AND h.ad_account_customer_id=a.customer_id LEFT JOIN monitoring_runs r ON r.workspace_id=a.workspace_id AND r.ad_account_customer_id=a.customer_id AND r.day=(SELECT MAX(day) FROM monitoring_runs r2 WHERE r2.workspace_id=a.workspace_id AND r2.ad_account_customer_id=a.customer_id) LEFT JOIN deep_checks d ON d.workspace_id=a.workspace_id AND d.ad_account_customer_id=a.customer_id LEFT JOIN google_failures g ON g.workspace_id=a.workspace_id AND g.ad_account_customer_id=a.customer_id LEFT JOIN deep_check_failures f ON f.workspace_id=a.workspace_id AND f.ad_account_customer_id=a.customer_id WHERE (?1 IS NOT NULL AND a.workspace_id=?1) OR (?1 IS NULL AND (h.succeeded_at IS NULL OR h.succeeded_at<?2-36*3600)) ORDER BY h.succeeded_at,a.workspace_id,a.customer_id LIMIT 100",vec![id.map(str::to_owned).into(),now.timestamp().into()]).await
    }
    pub async fn business_trend(&self, now: DateTime<Utc>) -> Result<Vec<BusinessPoint>> {
        self.rows("SELECT day,mrr_cents,paying FROM (SELECT day,mrr_cents,paying FROM business_days WHERE day<=? ORDER BY day DESC LIMIT 366) ORDER BY day",vec![day_number(now.date_naive()).into()]).await
    }
    pub async fn activation_trend(&self, now: DateTime<Utc>) -> Result<Vec<ActivationPoint>> {
        let day = day_number(now.date_naive());
        let monday = ((day + 3) / 7) * 7 - 3;
        self.rows("WITH RECURSIVE weeks(week) AS (SELECT ?1 UNION ALL SELECT week+7 FROM weeks WHERE week<?2), dates AS (SELECT ((created_at/86400+3)/7)*7-3 week,COUNT(*) registrations,0 connected,0 synced,0 assessed,0 exported FROM workspaces WHERE created_at BETWEEN ?1*86400 AND ?3 GROUP BY week UNION ALL SELECT ((day+3)/7)*7-3 week,0,COUNT(DISTINCT CASE WHEN kind='connected' THEN workspace_id END),COUNT(DISTINCT CASE WHEN kind='synced' THEN workspace_id END),COUNT(DISTINCT CASE WHEN kind='assessed' THEN workspace_id END),COUNT(DISTINCT CASE WHEN kind='exported' THEN workspace_id END) FROM workspace_activity WHERE day BETWEEN ?1 AND ?4 AND kind IN ('connected','synced','assessed','exported') GROUP BY week) SELECT w.week,COALESCE(SUM(d.registrations),0) registrations,COALESCE(SUM(d.connected),0) connected,COALESCE(SUM(d.synced),0) synced,COALESCE(SUM(d.assessed),0) assessed,COALESCE(SUM(d.exported),0) exported FROM weeks w LEFT JOIN dates d ON d.week=w.week GROUP BY w.week ORDER BY w.week",vec![(monday-28).into(),monday.into(),now.timestamp().into(),day.into()]).await
    }

    pub async fn weeks(&self, now: DateTime<Utc>) -> Result<Vec<Week>> {
        let day = day_number(now.date_naive());
        self.rows(&format!("SELECT ((day+3)/7)*7-3 week,kind,SUM(count) count,COUNT(DISTINCT workspace_id) workspaces FROM workspace_activity WHERE day BETWEEN ?1 AND ?2 AND kind IN ({}) GROUP BY week,kind ORDER BY week DESC,kind LIMIT 18",user_activity_kinds()),vec![(day-29).into(),day.into()]).await
    }
    pub async fn months(&self, now: DateTime<Utc>) -> Result<Vec<Month>> {
        self.rows("WITH changes AS (SELECT *,LAG(status) OVER (PARTITION BY workspace_id ORDER BY observed_at,status) previous_status,MIN(CASE WHEN status='active' THEN observed_at END) OVER(PARTITION BY workspace_id) first_active FROM subscription_changes WHERE observed_at<=?1), totals AS (SELECT strftime('%Y-%m',observed_at,'unixepoch') month,SUM(status='active' AND observed_at=first_active) new_subscriptions,SUM(status='canceled' AND COALESCE(previous_status,'')<>'canceled') cancellations,SUM(status IN ('past_due','unpaid') AND COALESCE(previous_status,'')<>status) payment_failures FROM changes GROUP BY month), months AS (SELECT month FROM totals UNION SELECT strftime('%Y-%m',day*86400,'unixepoch') month FROM business_days WHERE day*86400<=?1) SELECT m.month,COALESCE(t.new_subscriptions,0) new_subscriptions,COALESCE(t.cancellations,0) cancellations,COALESCE(t.payment_failures,0) payment_failures,(SELECT paying FROM business_days b WHERE strftime('%Y-%m',b.day*86400,'unixepoch')=m.month ORDER BY day LIMIT 1) opening_paying FROM months m LEFT JOIN totals t ON t.month=m.month ORDER BY m.month DESC LIMIT 24",vec![now.timestamp().into()]).await
    }
    pub async fn operations(&self, now: DateTime<Utc>) -> Result<Vec<OperationTotal>> {
        let day = day_number(now.date_naive());
        self.rows("SELECT operation,SUM(succeeded) succeeded,SUM(failed) failed,SUM(elapsed_ms) elapsed_ms FROM operation_days WHERE day BETWEEN ?1 AND ?2 GROUP BY operation ORDER BY operation LIMIT 9",vec![(day-13).into(),day.into()]).await
    }
    pub async fn ai_days(&self, now: DateTime<Utc>) -> Result<Vec<AiDay>> {
        let day = day_number(now.date_naive());
        self.rows("SELECT day,model,SUM(succeeded+failed) calls,SUM(failed) failed,SUM(elapsed_ms) elapsed_ms,CASE WHEN COUNT(input_tokens)=COUNT(*) THEN SUM(input_tokens) END input_tokens,CASE WHEN COUNT(output_tokens)=COUNT(*) THEN SUM(output_tokens) END output_tokens FROM operation_days WHERE day BETWEEN ?1 AND ?2 AND operation IN ('ai_update','ai_review','ai_research','ai_translation','ai_investigation') GROUP BY day,model ORDER BY day DESC,model LIMIT 200",vec![(day-13).into(),day.into()]).await
    }
    pub async fn queue(&self) -> Result<Option<Queue>> {
        let exists = self
            .db
            .query_one_raw(statement(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='sqlt_loco_queue'",
                vec![],
            ))
            .await?
            .is_some();
        if !exists {
            return Ok(None);
        }
        Ok(self.rows("SELECT COUNT(*) FILTER(WHERE status='failed' AND updated_at>=datetime(?1,'unixepoch','-7 days') AND updated_at<=datetime(?1,'unixepoch')) failed,COUNT(*) FILTER(WHERE status='processing') processing,MIN(CASE WHEN status='queued' AND run_at<=datetime(?1,'unixepoch') THEN run_at END) oldest_due FROM sqlt_loco_queue",vec![crate::settings::wall_clock().timestamp().into()]).await?.pop())
    }
    pub async fn failures(&self, queue: bool, limit: i64) -> Result<Vec<Failure>> {
        let queue_sql = if queue {
            " UNION ALL SELECT NULL,NULL,name,CAST(strftime('%s',updated_at) AS INTEGER),CAST(json_extract(task_data,'$.error') AS TEXT) FROM sqlt_loco_queue WHERE status='failed' AND json_extract(task_data,'$.error') IS NOT NULL"
        } else {
            ""
        };
        self.rows(&format!("SELECT f.workspace_id,u.email,f.customer_id,f.source,f.occurred_at,substr(f.error,1,2000) error FROM (SELECT workspace_id,ad_account_customer_id customer_id,'sync' source,COALESCE(collected_at,0) occurred_at,error FROM account_history WHERE error IS NOT NULL UNION ALL SELECT workspace_id,ad_account_customer_id,'google_request',failed_at,detail FROM google_failures UNION ALL SELECT workspace_id,ad_account_customer_id,'deep_check',day*86400,error FROM deep_check_failures{queue_sql}) f LEFT JOIN workspaces w ON w.id=f.workspace_id LEFT JOIN users u ON u.id=w.user_id ORDER BY f.occurred_at DESC,f.workspace_id,f.customer_id LIMIT ?"),vec![limit.clamp(1,30).into()]).await
    }
}
