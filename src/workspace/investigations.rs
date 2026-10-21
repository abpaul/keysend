use loco_rs::{Error, Result};
use sea_orm::{ConnectionTrait, DatabaseBackend, QueryResult, Statement, Value};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use super::WorkspaceStore;
use crate::{
    agent::{
        evidence::Evidence,
        operation::{Operation, Period},
        Failure, State,
    },
    database::begin_write,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Answered,
    Refused,
    Repeated,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InvestigationStep {
    pub ordinal: i64,
    pub round: i64,
    pub operation: Operation,
    pub outcome: Outcome,
    pub evidence: Vec<Evidence>,
    pub elapsed_ms: i64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvestigationUsage {
    pub model_calls: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Investigation {
    pub id: i64,
    pub follows: Option<i64>,
    pub question: String,
    pub accounts: Vec<String>,
    pub period: Period,
    pub state: State,
    pub failure: Option<Failure>,
    pub report: Option<serde_json::Value>,
    pub usage: InvestigationUsage,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub steps: Vec<InvestigationStep>,
}

pub struct InvestigationFinish {
    pub state: State,
    pub failure: Option<Failure>,
    pub report: Option<serde_json::Value>,
    pub usage: InvestigationUsage,
}

fn sql(query: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Sqlite, query, values)
}

fn name(value: impl Serialize) -> Result<String> {
    Ok(serde_json::to_value(value)?
        .as_str()
        .expect("enum name")
        .to_owned())
}

fn enum_column<T: DeserializeOwned>(row: &QueryResult, column: &str) -> Result<T> {
    Ok(serde_json::from_value(serde_json::Value::String(
        row.try_get("", column)?,
    ))?)
}

fn json_column<T: DeserializeOwned>(row: &QueryResult, column: &str) -> Result<T> {
    Ok(serde_json::from_str(&row.try_get::<String>("", column)?)?)
}

fn investigation(row: QueryResult) -> Result<Investigation> {
    let failure: Option<String> = row.try_get("", "failure")?;
    let report: Option<String> = row.try_get("", "report")?;
    Ok(Investigation {
        id: row.try_get("", "id")?,
        follows: row.try_get("", "follows")?,
        question: row.try_get("", "question")?,
        accounts: json_column(&row, "accounts")?,
        period: match row.try_get::<String>("", "period")?.as_str() {
            "last_7_days" => Period::Last7Days,
            "last_30_days" => Period::Last30Days,
            _ => return Err(Error::Message("Invalid investigation Window".into())),
        },
        state: enum_column(&row, "state")?,
        failure: failure
            .map(|value| serde_json::from_value(serde_json::Value::String(value)))
            .transpose()?,
        report: report
            .map(|value| serde_json::from_str(&value))
            .transpose()?,
        usage: InvestigationUsage {
            model_calls: row.try_get("", "model_calls")?,
            input_tokens: row.try_get("", "input_tokens")?,
            output_tokens: row.try_get("", "output_tokens")?,
        },
        created_at: row.try_get("", "created_at")?,
        started_at: row.try_get("", "started_at")?,
        finished_at: row.try_get("", "finished_at")?,
        steps: Vec::new(),
    })
}

fn predecessors(next: State) -> Result<String> {
    [
        State::Created,
        State::Gathering,
        State::Answering,
        State::Completed,
        State::Failed,
        State::Cancelled,
    ]
    .into_iter()
    .filter(|state| state.may_become(next))
    .map(|state| Ok(format!("'{}'", name(state)?)))
    .collect::<Result<Vec<_>>>()
    .map(|names| names.join(","))
}

impl WorkspaceStore<'_> {
    /// Returns `None` when the Workspace has reached its limit in this window.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_investigation(
        &self,
        question: &str,
        accounts: &[String],
        period: Period,
        follows: Option<i64>,
        now: i64,
        daily: i64,
        window_start: i64,
    ) -> Result<Option<i64>> {
        let txn = begin_write(self.db).await?;
        let count = txn.query_one_raw(sql(
            "SELECT COUNT(*) AS count FROM investigations WHERE workspace_id=? AND created_at>=?",
            vec![self.workspace.id().into(), window_start.into()],
        )).await?.expect("aggregate row").try_get::<i64>("", "count")?;
        if count >= daily {
            return Ok(None);
        }
        if let Some(id) = follows {
            if txn
                .query_one_raw(sql(
                    "SELECT id FROM investigations WHERE workspace_id=? AND id=?",
                    vec![self.workspace.id().into(), id.into()],
                ))
                .await?
                .is_none()
            {
                return Err(Error::NotFound);
            }
        }
        let row = txn.query_one_raw(sql("INSERT INTO investigations(workspace_id,follows,question,accounts,period,state,created_at) VALUES (?,?,?,?,?,'created',?) RETURNING id", vec![self.workspace.id().into(), follows.into(), question.into(), serde_json::to_string(accounts)?.into(), match period { Period::Last7Days => "last_7_days", Period::Last30Days => "last_30_days" }.into(), now.into()])).await?.expect("inserted investigation");
        let id = row.try_get("", "id")?;
        txn.commit().await?;
        Ok(Some(id))
    }

    pub async fn investigations_created_since(&self, since: i64) -> Result<i64> {
        let row = self.db.query_one_raw(sql("SELECT COUNT(*) AS count FROM investigations WHERE workspace_id=? AND created_at>=?", vec![self.workspace.id().into(), since.into()])).await?.expect("aggregate row");
        Ok(row.try_get("", "count")?)
    }

    pub async fn investigation(&self, id: i64) -> Result<Option<Investigation>> {
        let row = self
            .db
            .query_one_raw(sql(
                "SELECT * FROM investigations WHERE workspace_id=? AND id=?",
                vec![self.workspace.id().into(), id.into()],
            ))
            .await?;
        let Some(mut investigation) = row.map(investigation).transpose()? else {
            return Ok(None);
        };
        investigation.steps = self.investigation_steps(id).await?;
        Ok(Some(investigation))
    }

    async fn investigation_steps(&self, id: i64) -> Result<Vec<InvestigationStep>> {
        self.db.query_all_raw(sql("SELECT s.* FROM investigation_steps s JOIN investigations i ON i.id=s.investigation_id WHERE i.workspace_id=? AND i.id=? ORDER BY s.ordinal", vec![self.workspace.id().into(), id.into()])).await?.into_iter().map(|row| Ok(InvestigationStep {
            ordinal: row.try_get("", "ordinal")?, round: row.try_get("", "round")?,
            operation: json_column(&row, "operation")?, outcome: enum_column(&row, "outcome")?,
            evidence: json_column(&row, "evidence")?, elapsed_ms: row.try_get("", "elapsed_ms")?,
        })).collect()
    }

    pub async fn recent_investigations(&self, limit: u32) -> Result<Vec<Investigation>> {
        self.db.query_all_raw(sql("SELECT * FROM investigations WHERE workspace_id=? ORDER BY created_at DESC,id DESC LIMIT ?", vec![self.workspace.id().into(), limit.into()])).await?.into_iter().map(investigation).collect()
    }

    pub async fn investigation_thread(&self, id: i64) -> Result<Vec<Investigation>> {
        let mut investigations = self.db.query_all_raw(sql("WITH RECURSIVE ancestors(id,follows) AS (SELECT id,follows FROM investigations WHERE workspace_id=?1 AND id=?2 UNION ALL SELECT i.id,i.follows FROM investigations i JOIN ancestors a ON i.id=a.follows WHERE i.workspace_id=?1), thread(id) AS (SELECT id FROM ancestors WHERE follows IS NULL UNION ALL SELECT i.id FROM investigations i JOIN thread t ON i.follows=t.id WHERE i.workspace_id=?1) SELECT * FROM investigations WHERE workspace_id=?1 AND id IN (SELECT id FROM thread) ORDER BY created_at,id", vec![self.workspace.id().into(), id.into()])).await?.into_iter().map(investigation).collect::<Result<Vec<_>>>()?;
        for investigation in &mut investigations {
            investigation.steps = self.investigation_steps(investigation.id).await?;
        }
        Ok(investigations)
    }

    pub async fn claim_investigation(&self, id: i64, now: i64) -> Result<bool> {
        let allowed = predecessors(State::Gathering)?;
        Ok(self.db.execute_raw(sql(&format!("UPDATE investigations SET state='gathering',started_at=? WHERE workspace_id=? AND id=? AND state IN ({allowed})"), vec![now.into(), self.workspace.id().into(), id.into()])).await?.rows_affected() == 1)
    }

    /// A retry cannot resume a loop whose in-memory conversation was lost.
    pub async fn fail_interrupted_investigation(
        &self,
        id: i64,
        started_before: i64,
        now: i64,
    ) -> Result<bool> {
        let allowed = predecessors(State::Failed)?;
        Ok(self.db.execute_raw(sql(&format!("UPDATE investigations SET state='failed',failure='interrupted',finished_at=? WHERE workspace_id=? AND id=? AND state IN ('gathering','answering') AND state IN ({allowed}) AND started_at<?"), vec![now.into(), self.workspace.id().into(), id.into(), started_before.into()])).await?.rows_affected() == 1)
    }

    pub async fn save_investigation_step(&self, id: i64, step: &InvestigationStep) -> Result<bool> {
        Ok(self.db.execute_raw(sql("INSERT INTO investigation_steps(investigation_id,ordinal,round,operation,outcome,evidence,elapsed_ms) SELECT id,?,?,?,?,?,? FROM investigations WHERE workspace_id=? AND id=? ON CONFLICT(investigation_id,ordinal) DO NOTHING", vec![step.ordinal.into(), step.round.into(), serde_json::to_string(&step.operation)?.into(), name(step.outcome)?.into(), serde_json::to_string(&step.evidence)?.into(), step.elapsed_ms.into(), self.workspace.id().into(), id.into()])).await?.rows_affected() == 1)
    }

    pub async fn set_investigation_state(&self, id: i64, state: State) -> Result<bool> {
        let allowed = predecessors(state)?;
        if allowed.is_empty() {
            return Ok(false);
        }
        Ok(self.db.execute_raw(sql(&format!("UPDATE investigations SET state=? WHERE workspace_id=? AND id=? AND state IN ({allowed})"), vec![name(state)?.into(), self.workspace.id().into(), id.into()])).await?.rows_affected() == 1)
    }

    pub async fn finish_investigation(
        &self,
        id: i64,
        finish: &InvestigationFinish,
        now: i64,
    ) -> Result<bool> {
        if !finish.state.finished() {
            return Ok(false);
        }
        let allowed = predecessors(finish.state)?;
        Ok(self.db.execute_raw(sql(&format!("UPDATE investigations SET state=?,failure=?,report=?,model_calls=?,input_tokens=?,output_tokens=?,finished_at=? WHERE workspace_id=? AND id=? AND state IN ({allowed})"), vec![name(finish.state)?.into(), finish.failure.map(name).transpose()?.into(), finish.report.as_ref().map(serde_json::to_string).transpose()?.into(), finish.usage.model_calls.into(), finish.usage.input_tokens.into(), finish.usage.output_tokens.into(), now.into(), self.workspace.id().into(), id.into()])).await?.rows_affected() == 1)
    }

    pub async fn cancel_investigation(&self, id: i64, now: i64) -> Result<bool> {
        let allowed = predecessors(State::Cancelled)?;
        Ok(self.db.execute_raw(sql(&format!("UPDATE investigations SET state='cancelled',finished_at=? WHERE workspace_id=? AND id=? AND state IN ({allowed})"), vec![now.into(), self.workspace.id().into(), id.into()])).await?.rows_affected() == 1)
    }

    pub async fn investigation_cancelled(&self, id: i64) -> Result<bool> {
        Ok(self
            .db
            .query_one_raw(sql(
                "SELECT id FROM investigations WHERE workspace_id=? AND id=? AND state='cancelled'",
                vec![self.workspace.id().into(), id.into()],
            ))
            .await?
            .is_some())
    }
}
