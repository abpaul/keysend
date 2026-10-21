use chrono::{DateTime, Utc};
use loco_rs::Result;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement, Value};
use std::time::Duration;

use super::{history::day_number, WorkspaceStore};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    Viewed,
    Exported,
    Feedback,
    Connected,
    Synced,
    Assessed,
}

impl Activity {
    pub const ALL: [Self; 6] = [
        Self::Viewed,
        Self::Exported,
        Self::Feedback,
        Self::Connected,
        Self::Synced,
        Self::Assessed,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Viewed => "viewed",
            Self::Exported => "exported",
            Self::Feedback => "feedback",
            Self::Connected => "connected",
            Self::Synced => "synced",
            Self::Assessed => "assessed",
        }
    }

    pub const fn by_user(self) -> bool {
        matches!(self, Self::Viewed | Self::Exported | Self::Feedback)
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Operation {
    Sync,
    DeepCheck,
    Export,
    GoogleRequest,
    AiUpdate,
    AiReview,
    AiResearch,
    AiTranslation,
    AiInvestigation,
}

impl Operation {
    pub const ALL: [Self; 9] = [
        Self::Sync,
        Self::DeepCheck,
        Self::Export,
        Self::GoogleRequest,
        Self::AiUpdate,
        Self::AiReview,
        Self::AiResearch,
        Self::AiTranslation,
        Self::AiInvestigation,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sync => "sync",
            Self::DeepCheck => "deep_check",
            Self::Export => "export",
            Self::GoogleRequest => "google_request",
            Self::AiUpdate => "ai_update",
            Self::AiReview => "ai_review",
            Self::AiResearch => "ai_research",
            Self::AiTranslation => "ai_translation",
            Self::AiInvestigation => "ai_investigation",
        }
    }

    pub fn label(self) -> &'static str {
        let t = crate::language::english();
        match self {
            Self::Sync => t.admin_operation_sync,
            Self::DeepCheck => t.admin_dashboard_deep_check,
            Self::Export => t.admin_operation_export,
            Self::GoogleRequest => t.admin_operation_google,
            Self::AiUpdate => t.admin_operation_ai_update,
            Self::AiReview => t.admin_operation_ai_review,
            Self::AiResearch => t.admin_operation_ai_research,
            Self::AiTranslation => t.admin_operation_ai_translation,
            Self::AiInvestigation => t.admin_operation_ai_investigation,
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|op| op.as_str() == name)
    }

    pub fn from_schema(schema: &str) -> Option<Self> {
        match schema {
            "monitoring_update" => Some(Self::AiUpdate),
            "monitoring_review" => Some(Self::AiReview),
            "business_research" => Some(Self::AiResearch),
            "monitoring_translation" => Some(Self::AiTranslation),
            "agent_investigation" => Some(Self::AiInvestigation),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Usage {
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
}

fn sql(query: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Sqlite, query, values)
}

// Measurement is not product state: a failed recording must never fail a request or job.
pub fn recorded(result: Result<()>) {
    if let Err(error) = result {
        tracing::warn!(%error, "Measurement could not be recorded");
    }
}

pub async fn record(
    db: &DatabaseConnection,
    op: Operation,
    model: &str,
    ok: bool,
    elapsed: Duration,
    usage: Usage,
) {
    recorded(record_operation(db, op, model, ok, elapsed, usage).await);
}

async fn record_operation(
    db: &DatabaseConnection,
    op: Operation,
    model: &str,
    ok: bool,
    elapsed: Duration,
    usage: Usage,
) -> Result<()> {
    db.execute_raw(sql(
        "INSERT INTO operation_days(day,operation,model,succeeded,failed,elapsed_ms,input_tokens,output_tokens) VALUES(?,?,?,?,?,?,?,?) ON CONFLICT(day,operation,model) DO UPDATE SET succeeded=succeeded+excluded.succeeded,failed=failed+excluded.failed,elapsed_ms=elapsed_ms+excluded.elapsed_ms,input_tokens=CASE WHEN input_tokens IS NULL AND excluded.input_tokens IS NULL THEN NULL ELSE COALESCE(input_tokens,0)+COALESCE(excluded.input_tokens,0) END,output_tokens=CASE WHEN output_tokens IS NULL AND excluded.output_tokens IS NULL THEN NULL ELSE COALESCE(output_tokens,0)+COALESCE(excluded.output_tokens,0) END",
        vec![day_number(crate::settings::wall_clock().date_naive()).into(), op.as_str().into(), model.into(), i64::from(ok).into(), i64::from(!ok).into(), i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX).into(), usage.input_tokens.into(), usage.output_tokens.into()],
    )).await?;
    Ok(())
}

impl WorkspaceStore<'_> {
    pub async fn record_activity(&self, activity: Activity) {
        recorded(self.add_activity(activity).await);
    }

    async fn add_activity(&self, activity: Activity) -> Result<()> {
        self.db.execute_raw(sql(
            "INSERT INTO workspace_activity(workspace_id,day,kind,count) SELECT id,?,?,1 FROM workspaces WHERE id=? ON CONFLICT(workspace_id,day,kind) DO UPDATE SET count=count+1",
            vec![day_number(crate::settings::wall_clock().date_naive()).into(), activity.as_str().into(), self.workspace.id().into()],
        )).await?;
        Ok(())
    }

    pub async fn record_operation(
        &self,
        op: Operation,
        model: &str,
        ok: bool,
        elapsed: Duration,
        usage: Usage,
    ) {
        record(self.db, op, model, ok, elapsed, usage).await;
    }

    pub async fn remove_activity_before(&self, oldest: i64) -> Result<()> {
        self.db
            .execute_raw(sql(
                "DELETE FROM workspace_activity WHERE workspace_id=? AND day<?",
                vec![self.workspace.id().into(), oldest.into()],
            ))
            .await?;
        Ok(())
    }

    pub async fn activity_count(&self, activity: Activity) -> Result<i64> {
        let row = self.db.query_one_raw(sql("SELECT COALESCE(SUM(count),0) AS count FROM workspace_activity WHERE workspace_id=? AND kind=?", vec![self.workspace.id().into(), activity.as_str().into()])).await?.expect("aggregate row");
        Ok(row.try_get("", "count")?)
    }
}

pub(super) const BUSINESS_STATE: &str = "WITH state AS (
 SELECT w.id,w.created_at,w.sign_up_source,w.user_id,
 COALESCE(s.status='active' AND s.period_end>?1,0) AS paying,
 COALESCE(t.started=1 AND t.started_at<=?1 AND t.started_at+?2>?1 AND NOT (COALESCE(s.status='active' AND s.period_end>?1,0)),0) AS trial,
 COALESCE(s.status IN ('past_due','unpaid'),0) AS payment_due,
 COALESCE(s.status='active' AND s.period_end>?1 AND s.cancel_at_period_end=1,0) AS scheduled,
 COALESCE(s.status='canceled',0) AS canceled
 FROM workspaces w LEFT JOIN subscriptions s ON s.workspace_id=w.id LEFT JOIN workspace_trials t ON t.workspace_id=w.id
)";

pub(super) const BUSINESS_FIGURES: &str =
    "SELECT (SELECT COUNT(*) FROM users) AS users,COUNT(*) AS workspaces,
 (SELECT COUNT(*) FROM ad_accounts) AS ad_accounts,COALESCE(SUM(trial),0) AS trials,
 COALESCE(SUM(paying),0) AS paying,COALESCE(SUM(payment_due),0) AS payment_due,
 COALESCE(SUM(scheduled),0) AS scheduled_cancellations,COALESCE(SUM(canceled),0) AS canceled,
 COALESCE(SUM(paying),0)*?3 AS mrr_cents FROM state";

pub(super) fn business_values(now: DateTime<Utc>) -> Vec<Value> {
    vec![
        now.timestamp().into(),
        crate::monitoring::access::TRIAL_SECONDS.into(),
        crate::billing::SUBSCRIPTION_USD_CENTS.into(),
    ]
}

pub(super) fn user_activity_kinds() -> String {
    Activity::ALL
        .into_iter()
        .filter(|kind| kind.by_user())
        .map(|kind| format!("'{}'", kind.as_str()))
        .collect::<Vec<_>>()
        .join(",")
}

pub async fn snapshot(db: &DatabaseConnection, now: DateTime<Utc>) -> Result<()> {
    let mut values = business_values(now);
    values.push(day_number(now.date_naive()).into());
    db.execute_raw(sql(&format!("{BUSINESS_STATE} INSERT OR IGNORE INTO business_days(day,users,workspaces,ad_accounts,trials,paying,payment_due,scheduled_cancellations,canceled,mrr_cents) SELECT ?4,figures.* FROM ({BUSINESS_FIGURES}) figures"), values)).await?;
    Ok(())
}

pub async fn remove_before(db: &DatabaseConnection, oldest: i64) -> Result<()> {
    for table in ["operation_days", "business_days"] {
        db.execute_raw(sql(
            &format!("DELETE FROM {table} WHERE day<?"),
            vec![oldest.into()],
        ))
        .await?;
    }
    Ok(())
}

#[derive(Debug)]
pub struct OperationDay {
    pub day: i64,
    pub model: String,
    pub succeeded: i64,
    pub failed: i64,
    pub elapsed_ms: i64,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
}

pub async fn operation_days(db: &DatabaseConnection, op: Operation) -> Result<Vec<OperationDay>> {
    db.query_all_raw(sql("SELECT day,model,succeeded,failed,elapsed_ms,input_tokens,output_tokens FROM operation_days WHERE operation=? ORDER BY day,model", vec![op.as_str().into()])).await?.into_iter().map(|row| Ok(OperationDay {
        day: row.try_get("", "day")?, model: row.try_get("", "model")?, succeeded: row.try_get("", "succeeded")?, failed: row.try_get("", "failed")?, elapsed_ms: row.try_get("", "elapsed_ms")?, input_tokens: row.try_get("", "input_tokens")?, output_tokens: row.try_get("", "output_tokens")?,
    })).collect()
}

#[derive(Debug, PartialEq, Eq)]
pub struct BusinessDay {
    pub day: i64,
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

pub async fn business_days(db: &DatabaseConnection) -> Result<Vec<BusinessDay>> {
    db.query_all_raw(sql("SELECT * FROM business_days ORDER BY day", vec![]))
        .await?
        .into_iter()
        .map(|row| {
            Ok(BusinessDay {
                day: row.try_get("", "day")?,
                users: row.try_get("", "users")?,
                workspaces: row.try_get("", "workspaces")?,
                ad_accounts: row.try_get("", "ad_accounts")?,
                trials: row.try_get("", "trials")?,
                paying: row.try_get("", "paying")?,
                payment_due: row.try_get("", "payment_due")?,
                scheduled_cancellations: row.try_get("", "scheduled_cancellations")?,
                canceled: row.try_get("", "canceled")?,
                mrr_cents: row.try_get("", "mrr_cents")?,
            })
        })
        .collect()
}
