use loco_rs::Result;
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use serde::Serialize;
use serde_json::{json, Value};

use super::WorkspaceStore;
use crate::optimize::shadow::{Evaluation, Feedback};

#[derive(Clone, Debug, Serialize)]
pub struct Shadow {
    pub id: i64,
    pub ad_account: String,
    pub evaluation: Evaluation,
}

#[derive(Clone, Debug, Serialize)]
pub struct ShadowEvent {
    pub at: i64,
    pub kind: String,
    pub detail: Value,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct DecisionQuality {
    pub shadow_evaluations: u64,
    pub shadow_evaluations_without_action: u64,
    pub shadow_decisions: u64,
    pub shadow_abstentions: u64,
    pub useful_feedback: u64,
    pub consultant_overrides: u64,
    pub feedback_total: u64,
    pub manual_approvals: i64,
    pub manual_rejections: i64,
    pub execution_reservations: i64,
    pub execution_attempts: i64,
    pub verified_dispatches: i64,
    pub verified_executions: i64,
    pub policy_rejections: i64,
    pub conflicts: i64,
    pub observed_executions: i64,
}

impl WorkspaceStore<'_> {
    pub async fn decision_quality(&self) -> Result<DecisionQuality> {
        let row = self.db.query_one_raw(Statement::from_sql_and_values(DatabaseBackend::Sqlite,
            "SELECT (SELECT COUNT(*) FROM optimization_events WHERE workspace_id=?1 AND kind='approved') AS approved, (SELECT COUNT(*) FROM optimization_events WHERE workspace_id=?1 AND kind='rejected') AS rejected, (SELECT COUNT(*) FROM optimization_executions WHERE workspace_id=?1) AS reservations, (SELECT COUNT(DISTINCT proposal_id) FROM optimization_events WHERE workspace_id=?1 AND kind='dispatching') AS attempts, (SELECT COUNT(*) FROM optimization_executions x WHERE workspace_id=?1 AND verified=1 AND EXISTS (SELECT 1 FROM optimization_events e WHERE e.workspace_id=x.workspace_id AND e.proposal_id=x.proposal_id AND e.kind='dispatching')) AS verified_dispatches, (SELECT COUNT(*) FROM optimization_executions WHERE workspace_id=?1 AND verified=1) AS verified, (SELECT COUNT(*) FROM optimization_events WHERE workspace_id=?1 AND json_extract(detail,'$.refusal') IS NOT NULL) AS policy_rejections, (SELECT COUNT(*) FROM optimization_proposals WHERE workspace_id=?1 AND state='conflict') AS conflicts, (SELECT COUNT(DISTINCT e.proposal_id) FROM optimization_events e WHERE e.workspace_id=?1 AND e.kind='outcome' AND e.id=(SELECT MAX(latest.id) FROM optimization_events latest WHERE latest.workspace_id=e.workspace_id AND latest.proposal_id=e.proposal_id AND latest.kind='outcome') AND json_extract(e.detail,'$.state') NOT IN ('awaiting_data','data_quality','manual_review') AND json_extract(e.detail,'$.before_days')>=json_extract(e.detail,'$.days') AND json_extract(e.detail,'$.settled_days')>=json_extract(e.detail,'$.days') AND json_extract(e.detail,'$.tracking_reliable')=1 AND json_extract(e.detail,'$.confounding_changes')=0) AS observed",
            vec![self.workspace.id().into()]
        )).await?.ok_or_else(|| loco_rs::Error::Message("Decision measurements unavailable".into()))?;
        let mut result = DecisionQuality {
            manual_approvals: row.try_get("", "approved")?,
            manual_rejections: row.try_get("", "rejected")?,
            execution_reservations: row.try_get("", "reservations")?,
            execution_attempts: row.try_get("", "attempts")?,
            verified_dispatches: row.try_get("", "verified_dispatches")?,
            verified_executions: row.try_get("", "verified")?,
            policy_rejections: row.try_get("", "policy_rejections")?,
            conflicts: row.try_get("", "conflicts")?,
            observed_executions: row.try_get("", "observed")?,
            ..Default::default()
        };
        for row in self
            .db
            .query_all_raw(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "SELECT detail FROM optimization_shadows WHERE workspace_id=?",
                vec![self.workspace.id().into()],
            ))
            .await?
        {
            let evaluation: Evaluation =
                serde_json::from_str(&row.try_get::<String>("", "detail")?)?;
            result.shadow_evaluations += 1;
            if !evaluation
                .decisions
                .iter()
                .any(|decision| matches!(decision, crate::optimize::propose::Decision::Propose(_)))
            {
                result.shadow_evaluations_without_action += 1;
            }
            result.shadow_decisions += evaluation.decisions.len() as u64;
            result.shadow_abstentions += evaluation
                .decisions
                .iter()
                .filter(|decision| {
                    matches!(decision, crate::optimize::propose::Decision::Hold(_, _))
                })
                .count() as u64;
        }
        for row in self.db.query_all_raw(Statement::from_sql_and_values(DatabaseBackend::Sqlite,
            "SELECT e.detail FROM optimization_shadow_events e JOIN optimization_shadows s ON s.id=e.shadow_id WHERE s.workspace_id=? AND e.kind='feedback' AND e.id=(SELECT MAX(latest.id) FROM optimization_shadow_events latest WHERE latest.shadow_id=e.shadow_id AND latest.kind='feedback' AND json_extract(latest.detail,'$.item')=json_extract(e.detail,'$.item'))", vec![self.workspace.id().into()]
        )).await? {
            let detail: Value = serde_json::from_str(&row.try_get::<String>("", "detail")?)?;
            result.feedback_total += 1;
            if detail["feedback"] == "useful" { result.useful_feedback += 1; }
            else { result.consultant_overrides += 1; }
        }
        Ok(result)
    }
    pub async fn save_shadow(&self, id: &str, evaluation: &Evaluation) -> Result<()> {
        self.db.execute_raw(Statement::from_sql_and_values(DatabaseBackend::Sqlite,
            "INSERT INTO optimization_shadows(workspace_id,ad_account_customer_id,policy_version,data_day,at,detail) SELECT workspace_id,customer_id,?,?,?,? FROM ad_accounts WHERE workspace_id=? AND customer_id=? AND EXISTS (SELECT 1 FROM optimization_policies p WHERE p.workspace_id=ad_accounts.workspace_id AND p.ad_account_customer_id=ad_accounts.customer_id AND p.version=? AND json_extract(p.policy,'$.mode')='shadow') ON CONFLICT DO NOTHING",
            vec![evaluation.policy_version.into(), evaluation.data_day.into(), evaluation.at.into(), serde_json::to_string(evaluation)?.into(), self.workspace.id().into(), id.into(), evaluation.policy_version.into()]
        )).await?;
        Ok(())
    }

    pub async fn shadows(&self, account: Option<&str>, limit: u32) -> Result<Vec<Shadow>> {
        self.db.query_all_raw(Statement::from_sql_and_values(DatabaseBackend::Sqlite,
            "SELECT id,ad_account_customer_id,detail FROM optimization_shadows WHERE workspace_id=? AND (? IS NULL OR ad_account_customer_id=?) ORDER BY at DESC,id DESC LIMIT ?",
            vec![self.workspace.id().into(), account.map(str::to_owned).into(), account.map(str::to_owned).into(), limit.into()]
        )).await?.into_iter().map(|row| Ok(Shadow {
            id: row.try_get("", "id")?,
            ad_account: row.try_get("", "ad_account_customer_id")?,
            evaluation: serde_json::from_str(&row.try_get::<String>("", "detail")?)?,
        })).collect()
    }

    pub async fn shadow(&self, id: i64) -> Result<Option<Shadow>> {
        let row = self.db.query_one_raw(Statement::from_sql_and_values(DatabaseBackend::Sqlite,
            "SELECT id,ad_account_customer_id,detail FROM optimization_shadows WHERE workspace_id=? AND id=?",
            vec![self.workspace.id().into(), id.into()]
        )).await?;
        row.map(|row| {
            Ok(Shadow {
                id: row.try_get("", "id")?,
                ad_account: row.try_get("", "ad_account_customer_id")?,
                evaluation: serde_json::from_str(&row.try_get::<String>("", "detail")?)?,
            })
        })
        .transpose()
    }

    pub async fn latest_shadow_observation(&self, id: i64) -> Result<Option<ShadowEvent>> {
        self.db.query_one_raw(Statement::from_sql_and_values(DatabaseBackend::Sqlite,
            "SELECT e.at,e.kind,e.detail FROM optimization_shadow_events e JOIN optimization_shadows s ON s.id=e.shadow_id WHERE s.workspace_id=? AND s.id=? AND e.kind='observation' ORDER BY e.id DESC LIMIT 1",
            vec![self.workspace.id().into(), id.into()]
        )).await?.map(|row| Ok(ShadowEvent {
            at: row.try_get("", "at")?, kind: row.try_get("", "kind")?,
            detail: serde_json::from_str(&row.try_get::<String>("", "detail")?)?,
        })).transpose()
    }

    pub async fn shadow_events(&self, id: i64) -> Result<Vec<ShadowEvent>> {
        self.db.query_all_raw(Statement::from_sql_and_values(DatabaseBackend::Sqlite,
            "SELECT e.at,e.kind,e.detail FROM optimization_shadow_events e JOIN optimization_shadows s ON s.id=e.shadow_id WHERE s.workspace_id=? AND s.id=? ORDER BY e.id DESC LIMIT 100",
            vec![self.workspace.id().into(), id.into()]
        )).await?.into_iter().map(|row| Ok(ShadowEvent {
            at: row.try_get("", "at")?, kind: row.try_get("", "kind")?,
            detail: serde_json::from_str(&row.try_get::<String>("", "detail")?)?,
        })).collect()
    }

    pub async fn record_shadow_feedback(
        &self,
        id: i64,
        item: usize,
        feedback: Feedback,
        note: &str,
        now: i64,
    ) -> Result<bool> {
        let Some(shadow) = self.shadow(id).await? else {
            return Ok(false);
        };
        if item >= shadow.evaluation.decisions.len() || note.chars().count() > 1000 {
            return Ok(false);
        }
        self.record_shadow_event(
            id,
            "feedback",
            &json!({"item": item, "feedback": feedback, "note": note}),
            now,
        )
        .await
    }

    pub async fn record_shadow_event(
        &self,
        id: i64,
        kind: &str,
        detail: &Value,
        now: i64,
    ) -> Result<bool> {
        Ok(self.db.execute_raw(Statement::from_sql_and_values(DatabaseBackend::Sqlite,
            "INSERT INTO optimization_shadow_events(shadow_id,at,kind,detail) SELECT id,?1,?2,?3 FROM optimization_shadows WHERE workspace_id=?4 AND id=?5 AND NOT EXISTS (SELECT 1 FROM (SELECT detail FROM optimization_shadow_events WHERE shadow_id=?5 AND kind=?2 AND (?2!='feedback' OR json_extract(detail,'$.item')=json_extract(?3,'$.item')) ORDER BY id DESC LIMIT 1) WHERE detail=?3)",
            vec![now.into(), kind.into(), detail.to_string().into(), self.workspace.id().into(), id.into()]
        )).await?.rows_affected() == 1)
    }
}

#[cfg(test)]
pub(crate) async fn shadow_adapter_database(now: i64) -> Result<sea_orm::DatabaseConnection> {
    use migration::{Migrator, MigratorTrait};
    let db = sea_orm::Database::connect("sqlite::memory:").await?;
    Migrator::up(&db, None).await?;
    for sql in [
        "INSERT INTO users(id,google_subject_id,email,name) VALUES ('u','g','user@example.com','User')",
        "INSERT INTO workspaces(id,user_id) VALUES ('w','u')",
        "INSERT INTO google_connections(id,workspace_id,google_subject_id,email,encrypted_refresh_token) VALUES ('c','w','g','user@example.com','sealed')",
        "INSERT INTO ad_accounts(id,workspace_id,google_connection_id,customer_id,name) VALUES ('a','w','c','1234567890','Ad Account')",
    ] { db.execute_unprepared(sql).await?; }
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Sqlite,
        "INSERT INTO workspace_trials(workspace_id,started_at) VALUES ('w',?) ON CONFLICT(workspace_id) DO UPDATE SET started_at=excluded.started_at",
        vec![now.into()],
    ))
    .await?;
    Ok(db)
}
