//! The rows of Optimize: Optimization policy versions, the Workspace stop, Campaign budgets,
//! Proposals, executions, the append-only history and the last evaluation of each Ad Account.
//!
//! A Proposal row never changes its change or its fingerprint (a trigger refuses it); only its
//! state moves, along `ProposalState::may_become`, in SQL. Execution claims a Proposal in one
//! write transaction that also counts the changes and raises of the day and refuses a second
//! change in the same Ad Account while one is running.

use std::collections::{BTreeMap, BTreeSet};

use loco_rs::{Error, Result};
use sea_orm::{ConnectionTrait, DatabaseBackend, QueryResult, Statement, Value};
use serde::Serialize;

use super::WorkspaceStore;
use crate::{
    database::begin_write,
    optimize::{
        policy::{Policy, Refusal, Standing, Versioned},
        propose::Budget,
        Approval, Draft, Mode, Proposal, ProposalState,
    },
};

fn sql(query: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Sqlite, query, values)
}

/// Counts the changes of an Ad Account that failed, conflicted or stayed uncertain since a time.
fn failures(values: Vec<Value>) -> Statement {
    sql(
        "SELECT COUNT(*) AS n FROM optimization_executions e JOIN optimization_proposals p ON p.id=e.proposal_id WHERE e.workspace_id=? AND e.ad_account_customer_id=? AND e.started_at>=? AND p.state IN ('failed','uncertain','conflict')",
        values,
    )
}

fn states(states: &[ProposalState]) -> String {
    states
        .iter()
        .map(|state| format!("'{}'", state.name()))
        .collect::<Vec<_>>()
        .join(",")
}

fn proposal(row: QueryResult) -> Result<Proposal> {
    let state: String = row.try_get("", "state")?;
    Ok(Proposal {
        id: row.try_get("", "id")?,
        ad_account: row.try_get("", "ad_account_customer_id")?,
        draft: serde_json::from_str(&row.try_get::<String>("", "draft")?)?,
        fingerprint: row.try_get("", "fingerprint")?,
        state: ProposalState::from_name(&state)
            .ok_or_else(|| Error::Message("Invalid Proposal state".into()))?,
        decided_at: row.try_get("", "decided_at")?,
        approved: row.try_get("", "approved_fingerprint")?,
    })
}

/// One line of the optimization history.
#[derive(Clone, Debug, Serialize)]
pub struct Event {
    pub id: i64,
    pub ad_account: String,
    pub proposal_id: Option<i64>,
    pub at: i64,
    pub kind: String,
    pub detail: serde_json::Value,
}

/// The execution of a Proposal.
#[derive(Clone, Debug, Serialize)]
pub struct Execution {
    pub proposal_id: i64,
    pub ad_account: String,
    pub day: i64,
    pub automatic: bool,
    pub before: Option<serde_json::Value>,
    pub after: Option<serde_json::Value>,
    pub request_id: Option<String>,
    pub error: Option<String>,
    pub verified: bool,
    pub started_at: i64,
    pub finished_at: Option<i64>,
}

/// Why a Proposal could not be claimed for execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Claim {
    Claimed,
    /// The Proposal is not approved, or another execution claimed it first.
    NotApproved,
    /// Another change of the same Ad Account is running or uncertain.
    Busy,
    DailyChanges,
    DailyRaise,
}

pub struct Finish<'a> {
    pub state: ProposalState,
    pub before: Option<&'a serde_json::Value>,
    pub after: Option<&'a serde_json::Value>,
    pub request_id: Option<&'a str>,
    /// What Google Ads answered, or why no read settled the change.
    pub error: Option<&'a str>,
    /// The policy refused the change after the claim; nothing was sent.
    pub refusal: Option<Refusal>,
    pub verified: bool,
    /// Suspends an active Autopilot of the Ad Account in the same transaction, only when this
    /// finish moves the Proposal, so a worker that lost the change never suspends a policy
    /// that the User saved since.
    pub suspend: Option<Suspend<'a>>,
}

#[derive(Clone, Copy, Debug)]
pub enum Suspend<'a> {
    Because(&'a str),
    /// When the changes that failed, conflicted or stayed uncertain since a time reach a limit.
    AfterFailures {
        since: i64,
        limit: u32,
    },
}

/// The policy reason and execution error of a change that the User reported as not applied.
pub const RECOVERED: &str = "uncertain_recovery";
pub const NOT_APPLIED: &str = "operator_reported_not_applied";

/// What the User reported about an Uncertain change, and who reported it.
pub struct Report<'a> {
    pub note: &'a str,
    pub user_id: &'a str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recovery {
    Recovered,
    /// The same report was already recorded for this Proposal.
    Already,
    /// The Proposal is not Uncertain, or the fingerprint is not its own.
    NotUncertain,
}

const RUNNING: [ProposalState; 2] = [ProposalState::Applying, ProposalState::Uncertain];

impl WorkspaceStore<'_> {
    fn account_values(&self, id: &str) -> Vec<Value> {
        vec![self.workspace.id().into(), id.into()]
    }

    /// The current policy of an Ad Account: its newest version, or Investigate when the User
    /// never set one.
    pub async fn optimization_policy(&self, id: &str) -> Result<Versioned> {
        Ok(self
            .optimization_policy_history(id, 1)
            .await?
            .into_iter()
            .next()
            .unwrap_or(Versioned {
                version: 0,
                policy: Policy::default(),
                standing: Standing::Active,
                reason: String::new(),
                created_at: 0,
            }))
    }

    pub async fn optimization_policy_history(
        &self,
        id: &str,
        limit: u32,
    ) -> Result<Vec<Versioned>> {
        let mut values = self.account_values(id);
        values.push(limit.into());
        self.db
            .query_all_raw(sql(
                "SELECT version,policy,standing,reason,created_at FROM optimization_policies WHERE workspace_id=? AND ad_account_customer_id=? ORDER BY version DESC LIMIT ?",
                values,
            ))
            .await?
            .into_iter()
            .map(|row| {
                Ok(Versioned {
                    version: row.try_get("", "version")?,
                    policy: serde_json::from_str(&row.try_get::<String>("", "policy")?)?,
                    standing: Standing::from_name(&row.try_get::<String>("", "standing")?)
                        .ok_or_else(|| Error::Message("Invalid policy standing".into()))?,
                    reason: row.try_get("", "reason")?,
                    created_at: row.try_get("", "created_at")?,
                })
            })
            .collect()
    }

    /// The current policy of each Ad Account of the Workspace that has one.
    pub async fn optimization_policies(&self) -> Result<BTreeMap<String, Versioned>> {
        self.db
            .query_all_raw(sql(
                "SELECT p.ad_account_customer_id,p.version,p.policy,p.standing,p.reason,p.created_at FROM optimization_policies p WHERE p.workspace_id=?1 AND p.version=(SELECT MAX(version) FROM optimization_policies q WHERE q.workspace_id=?1 AND q.ad_account_customer_id=p.ad_account_customer_id)",
                vec![self.workspace.id().into()],
            ))
            .await?
            .into_iter()
            .map(|row| {
                Ok((
                    row.try_get("", "ad_account_customer_id")?,
                    Versioned {
                        version: row.try_get("", "version")?,
                        policy: serde_json::from_str(&row.try_get::<String>("", "policy")?)?,
                        standing: Standing::from_name(&row.try_get::<String>("", "standing")?)
                            .ok_or_else(|| Error::Message("Invalid policy standing".into()))?,
                        reason: row.try_get("", "reason")?,
                        created_at: row.try_get("", "created_at")?,
                    },
                ))
            })
            .collect()
    }

    /// The last evaluation of each Ad Account of the Workspace: when, and its holds.
    pub async fn optimization_evaluations(
        &self,
    ) -> Result<BTreeMap<String, (i64, serde_json::Value)>> {
        self.db
            .query_all_raw(sql(
                "SELECT ad_account_customer_id,evaluated_at,holds FROM optimization_evaluations WHERE workspace_id=?",
                vec![self.workspace.id().into()],
            ))
            .await?
            .into_iter()
            .map(|row| {
                Ok((
                    row.try_get("", "ad_account_customer_id")?,
                    (
                        row.try_get("", "evaluated_at")?,
                        serde_json::from_str(&row.try_get::<String>("", "holds")?)?,
                    ),
                ))
            })
            .collect()
    }

    /// The currency of each Ad Account of the Workspace.
    pub async fn ad_account_currencies(&self) -> Result<BTreeMap<String, String>> {
        self.db
            .query_all_raw(sql(
                "SELECT ad_account_customer_id,currency FROM ad_account_metadata WHERE workspace_id=?",
                vec![self.workspace.id().into()],
            ))
            .await?
            .into_iter()
            .map(|row| Ok((row.try_get("", "ad_account_customer_id")?, row.try_get("", "currency")?)))
            .collect()
    }

    /// Stores a new version of the policy with its standing, and the event in the history,
    /// in one transaction. Read-only modes retire Open and Approved Proposals with history,
    /// preserving executions that already started. Returns the version.
    pub async fn save_optimization_policy(
        &self,
        id: &str,
        policy: &Policy,
        standing: Standing,
        reason: &str,
        now: i64,
    ) -> Result<i64> {
        if !policy.valid() {
            return Err(Error::Message("Invalid Optimization policy".into()));
        }
        let txn = begin_write(self.db).await?;
        let owned = txn
            .query_one_raw(sql(
                "SELECT customer_id FROM ad_accounts WHERE workspace_id=? AND customer_id=?",
                self.account_values(id),
            ))
            .await?;
        if owned.is_none() {
            return Err(Error::NotFound);
        }
        let version = self
            .policy_in(&txn, id, policy, standing, reason, now)
            .await?;
        txn.commit().await?;
        Ok(version)
    }

    async fn policy_in(
        &self,
        txn: &impl ConnectionTrait,
        id: &str,
        policy: &Policy,
        standing: Standing,
        reason: &str,
        now: i64,
    ) -> Result<i64> {
        let row = txn
            .query_one_raw(sql(
                "INSERT INTO optimization_policies(workspace_id,ad_account_customer_id,version,policy,standing,reason,created_at) SELECT ?1,?2,COALESCE(MAX(version),0)+1,?3,?4,?5,?6 FROM optimization_policies WHERE workspace_id=?1 AND ad_account_customer_id=?2 RETURNING version",
                vec![
                    self.workspace.id().into(),
                    id.into(),
                    serde_json::to_string(policy)?.into(),
                    standing.name().into(),
                    reason.into(),
                    now.into(),
                ],
            ))
            .await?
            .ok_or_else(|| Error::Message("Policy version was not stored".into()))?;
        let version: i64 = row.try_get("", "version")?;
        txn.execute_raw(sql(
            "INSERT INTO optimization_events(workspace_id,ad_account_customer_id,proposal_id,at,kind,detail) VALUES(?,?,NULL,?,'policy',?)",
            vec![
                self.workspace.id().into(),
                id.into(),
                now.into(),
                serde_json::json!({"version": version, "mode": policy.mode, "standing": standing.name(), "reason": reason}).to_string().into(),
            ],
        ))
        .await?;
        if matches!(policy.mode, Mode::Shadow | Mode::Investigate) {
            let retired = txn.query_all_raw(sql(
                "UPDATE optimization_proposals SET state=CASE state WHEN 'open' THEN 'superseded' ELSE 'expired' END,decided_at=COALESCE(decided_at,?) WHERE workspace_id=? AND ad_account_customer_id=? AND state IN ('open','approved') RETURNING id,state",
                vec![now.into(), self.workspace.id().into(), id.into()],
            )).await?;
            for row in retired {
                self.event_in(txn, row.try_get("", "id")?, &row.try_get::<String>("", "state")?,
                    &serde_json::json!({"policy_version": version, "mode": policy.mode, "reason": "mode_changed"}), now).await?;
            }
        }
        Ok(version)
    }

    /// The User's report that Google Ads confirmed an Uncertain change was not applied and
    /// will not be. KeySend cannot verify it, so in one transaction the Proposal becomes
    /// Failed with the report as unverified evidence, and the Ad Account moves to Investigate,
    /// suspended, which retires its other Open and Approved Proposals. Nothing is sent to
    /// Google Ads. Only an Uncertain Proposal with the fingerprint the page showed moves; the
    /// same report again changes nothing.
    pub async fn recover_uncertain(
        &self,
        proposal: i64,
        fingerprint: &str,
        report: &Report<'_>,
        now: i64,
    ) -> Result<Recovery> {
        let txn = begin_write(self.db).await?;
        let Some(row) = txn
            .query_one_raw(sql(
                "UPDATE optimization_proposals SET state='failed' WHERE workspace_id=? AND id=? AND state='uncertain' AND fingerprint=? RETURNING ad_account_customer_id",
                vec![self.workspace.id().into(), proposal.into(), fingerprint.into()],
            ))
            .await?
        else {
            let recovered = txn
                .query_one_raw(sql(
                    "SELECT 1 FROM optimization_events e JOIN optimization_proposals p ON p.id=e.proposal_id AND p.workspace_id=e.workspace_id WHERE e.workspace_id=? AND e.proposal_id=? AND e.kind='failed' AND json_extract(e.detail,'$.recovery')=? AND p.fingerprint=?",
                    vec![self.workspace.id().into(), proposal.into(), NOT_APPLIED.into(), fingerprint.into()],
                ))
                .await?
                .is_some();
            return Ok(if recovered { Recovery::Already } else { Recovery::NotUncertain });
        };
        let id: String = row.try_get("", "ad_account_customer_id")?;
        txn.execute_raw(sql(
            "UPDATE optimization_executions SET error=?,verified=0,finished_at=? WHERE workspace_id=? AND proposal_id=?",
            vec![NOT_APPLIED.into(), now.into(), self.workspace.id().into(), proposal.into()],
        ))
        .await?;
        self.event_in(
            &txn,
            proposal,
            ProposalState::Failed.name(),
            &serde_json::json!({
                "recovery": NOT_APPLIED,
                "verified": false,
                "confirmed": true,
                "note": report.note,
                "user_id": report.user_id,
                "fingerprint": fingerprint,
            }),
            now,
        )
        .await?;
        let current = txn
            .query_one_raw(sql(
                "SELECT policy FROM optimization_policies WHERE workspace_id=? AND ad_account_customer_id=? ORDER BY version DESC LIMIT 1",
                self.account_values(&id),
            ))
            .await?
            .map(|row| row.try_get::<String>("", "policy"))
            .transpose()?
            .map(|policy| serde_json::from_str::<Policy>(&policy))
            .transpose()?
            .unwrap_or_default();
        let policy = Policy {
            mode: Mode::Investigate,
            ..current
        };
        self.policy_in(&txn, &id, &policy, Standing::Suspended, RECOVERED, now)
            .await?;
        txn.commit().await?;
        Ok(Recovery::Recovered)
    }

    /// Whether the User stopped every change of the Workspace.
    pub async fn optimization_stopped(&self) -> Result<bool> {
        Ok(self
            .db
            .query_one_raw(sql(
                "SELECT stopped_at FROM optimization_stops WHERE workspace_id=?",
                vec![self.workspace.id().into()],
            ))
            .await?
            .is_some())
    }

    /// Stops or resumes every change of the Workspace, with an event in the history of each
    /// Ad Account. A stop also fails the approved changes that have not started.
    pub async fn set_optimization_stopped(&self, stopped: bool, now: i64) -> Result<()> {
        let txn = begin_write(self.db).await?;
        if stopped {
            txn.execute_raw(sql(
                "INSERT INTO optimization_stops(workspace_id,stopped_at) VALUES(?,?) ON CONFLICT(workspace_id) DO NOTHING",
                vec![self.workspace.id().into(), now.into()],
            ))
            .await?;
            txn.execute_raw(sql(
                "INSERT INTO optimization_events(workspace_id,ad_account_customer_id,proposal_id,at,kind,detail) SELECT workspace_id,ad_account_customer_id,id,?,'failed','{\"refusal\":\"stopped\"}' FROM optimization_proposals WHERE workspace_id=? AND state='approved'",
                vec![now.into(), self.workspace.id().into()],
            ))
            .await?;
            txn.execute_raw(sql(
                "UPDATE optimization_proposals SET state='failed' WHERE workspace_id=? AND state='approved'",
                vec![self.workspace.id().into()],
            ))
            .await?;
        } else {
            txn.execute_raw(sql(
                "DELETE FROM optimization_stops WHERE workspace_id=?",
                vec![self.workspace.id().into()],
            ))
            .await?;
        }
        txn.execute_raw(sql(
            "INSERT INTO optimization_events(workspace_id,ad_account_customer_id,proposal_id,at,kind,detail) SELECT workspace_id,customer_id,NULL,?,?,'{}' FROM ad_accounts WHERE workspace_id=?",
            vec![
                now.into(),
                if stopped { "stopped" } else { "resumed" }.into(),
                self.workspace.id().into(),
            ],
        ))
        .await?;
        txn.commit().await?;
        Ok(())
    }

    /// Replaces the budgets of the Campaigns of an Ad Account, from the daily collection.
    pub async fn save_campaign_budgets(&self, id: &str, budgets: &[(i64, Budget)]) -> Result<()> {
        let txn = begin_write(self.db).await?;
        txn.execute_raw(sql(
            "DELETE FROM campaign_budgets WHERE workspace_id=? AND ad_account_customer_id=?",
            self.account_values(id),
        ))
        .await?;
        for (campaign, budget) in budgets {
            txn.execute_raw(sql(
                "INSERT INTO campaign_budgets(workspace_id,ad_account_customer_id,campaign_id,budget_id,shared,campaigns) SELECT workspace_id,customer_id,?,?,?,? FROM ad_accounts WHERE workspace_id=? AND customer_id=?",
                vec![
                    (*campaign).into(),
                    budget.budget_id.into(),
                    i64::from(budget.shared).into(),
                    budget.campaigns.into(),
                    self.workspace.id().into(),
                    id.into(),
                ],
            ))
            .await?;
        }
        txn.commit().await?;
        Ok(())
    }

    pub async fn campaign_budgets(&self, id: &str) -> Result<BTreeMap<i64, Budget>> {
        self.db
            .query_all_raw(sql(
                "SELECT campaign_id,budget_id,shared,campaigns FROM campaign_budgets WHERE workspace_id=? AND ad_account_customer_id=?",
                self.account_values(id),
            ))
            .await?
            .into_iter()
            .map(|row| {
                Ok((
                    row.try_get("", "campaign_id")?,
                    Budget {
                        budget_id: row.try_get("", "budget_id")?,
                        shared: row.try_get::<i64>("", "shared")? == 1,
                        campaigns: row.try_get("", "campaigns")?,
                    },
                ))
            })
            .collect()
    }

    /// Stores a Proposal with its event. Returns `None` when a Proposal with the same key is
    /// still open, approved or running in the Ad Account.
    pub async fn save_proposal(&self, id: &str, draft: &Draft, now: i64) -> Result<Option<i64>> {
        let fingerprint = draft.fingerprint(id);
        let txn = begin_write(self.db).await?;
        let row = txn
            .query_one_raw(sql(
                "INSERT INTO optimization_proposals(workspace_id,ad_account_customer_id,key,draft,fingerprint,approval,state,created_at,expires_at) SELECT workspace_id,customer_id,?,?,?,?,'open',?,? FROM ad_accounts WHERE workspace_id=? AND customer_id=? ON CONFLICT DO NOTHING RETURNING id",
                vec![
                    draft.key.clone().into(),
                    serde_json::to_string(draft)?.into(),
                    fingerprint.into(),
                    match draft.approval {
                        Approval::Required => "required",
                        Approval::Automatic => "automatic",
                    }
                    .into(),
                    draft.created_at.into(),
                    draft.expires_at.into(),
                    self.workspace.id().into(),
                    id.into(),
                ],
            ))
            .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let proposal: i64 = row.try_get("", "id")?;
        txn.execute_raw(sql(
            "INSERT INTO optimization_events(workspace_id,ad_account_customer_id,proposal_id,at,kind,detail) VALUES(?,?,?,?,'proposed',?)",
            vec![
                self.workspace.id().into(),
                id.into(),
                proposal.into(),
                now.into(),
                serde_json::json!({"action": draft.action, "approval": draft.approval, "policy_version": draft.policy_version, "signal": draft.signal.kind}).to_string().into(),
            ],
        ))
        .await?;
        txn.commit().await?;
        Ok(Some(proposal))
    }

    pub async fn proposal(&self, proposal: i64) -> Result<Option<Proposal>> {
        self.db
            .query_one_raw(sql(
                "SELECT * FROM optimization_proposals WHERE workspace_id=? AND id=?",
                vec![self.workspace.id().into(), proposal.into()],
            ))
            .await?
            .map(self::proposal)
            .transpose()
    }

    /// Proposals of the Workspace, or of one Ad Account, in these states, newest first.
    pub async fn proposals(
        &self,
        id: Option<&str>,
        in_states: &[ProposalState],
        limit: u32,
    ) -> Result<Vec<Proposal>> {
        let mut values = vec![self.workspace.id().into()];
        let account = match id {
            Some(id) => {
                values.push(id.into());
                " AND ad_account_customer_id=?"
            }
            None => "",
        };
        values.push(limit.into());
        self.db
            .query_all_raw(sql(
                &format!("SELECT * FROM optimization_proposals WHERE workspace_id=?{account} AND state IN ({}) ORDER BY created_at DESC,id DESC LIMIT ?", states(in_states)),
                values,
            ))
            .await?
            .into_iter()
            .map(self::proposal)
            .collect()
    }

    /// Keys of the Proposals that are open, approved or running in an Ad Account.
    pub async fn open_proposal_keys(&self, id: &str) -> Result<BTreeSet<String>> {
        self.db
            .query_all_raw(sql(
                "SELECT key FROM optimization_proposals WHERE workspace_id=? AND ad_account_customer_id=? AND state IN ('open','approved','applying','uncertain')",
                self.account_values(id),
            ))
            .await?
            .into_iter()
            .map(|row| Ok(row.try_get("", "key")?))
            .collect()
    }

    pub async fn rejected_proposal_keys(&self, id: &str, since: i64) -> Result<BTreeSet<String>> {
        let mut values = self.account_values(id);
        values.push(since.into());
        self.db
            .query_all_raw(sql(
                "SELECT key FROM optimization_proposals WHERE workspace_id=? AND ad_account_customer_id=? AND state='rejected' AND decided_at>=?",
                values,
            ))
            .await?
            .into_iter()
            .map(|row| Ok(row.try_get("", "key")?))
            .collect()
    }

    /// Moves a Proposal to a state along the allowed transitions, with an event. Returns
    /// whether it moved.
    pub async fn set_proposal_state(
        &self,
        proposal: i64,
        next: ProposalState,
        detail: serde_json::Value,
        now: i64,
    ) -> Result<bool> {
        let allowed = states(
            &ProposalState::ALL
                .into_iter()
                .filter(|state| !RUNNING.contains(state) && state.may_become(next))
                .collect::<Vec<_>>(),
        );
        if next == ProposalState::Applying || allowed.is_empty() {
            return Ok(false);
        }
        let txn = begin_write(self.db).await?;
        let moved = txn
            .execute_raw(sql(
                &format!("UPDATE optimization_proposals SET state=?,decided_at=COALESCE(decided_at,?) WHERE workspace_id=? AND id=? AND state IN ({allowed})"),
                vec![
                    next.name().into(),
                    now.into(),
                    self.workspace.id().into(),
                    proposal.into(),
                ],
            ))
            .await?
            .rows_affected()
            == 1;
        if moved {
            self.event_in(&txn, proposal, next.name(), &detail, now)
                .await?;
        }
        txn.commit().await?;
        Ok(moved)
    }

    async fn event_in(
        &self,
        txn: &impl ConnectionTrait,
        proposal: i64,
        kind: &str,
        detail: &serde_json::Value,
        now: i64,
    ) -> Result<()> {
        txn.execute_raw(sql(
            "INSERT INTO optimization_events(workspace_id,ad_account_customer_id,proposal_id,at,kind,detail) SELECT workspace_id,ad_account_customer_id,id,?,?,? FROM optimization_proposals WHERE workspace_id=? AND id=?",
            vec![
                now.into(),
                kind.into(),
                detail.to_string().into(),
                self.workspace.id().into(),
                proposal.into(),
            ],
        ))
        .await?;
        Ok(())
    }

    /// The User's Approval of the exact Proposal that the page showed: it moves only an open,
    /// unexpired Proposal whose fingerprint is the submitted one, and stores that fingerprint.
    pub async fn approve_proposal(
        &self,
        proposal: i64,
        fingerprint: &str,
        now: i64,
    ) -> Result<bool> {
        self.authorize(proposal, fingerprint, "approved", now).await
    }

    /// The Autopilot's authorization of a Proposal that its policy lets run without the User.
    pub async fn authorize_proposal(
        &self,
        proposal: i64,
        fingerprint: &str,
        now: i64,
    ) -> Result<bool> {
        self.authorize(proposal, fingerprint, "authorized", now)
            .await
    }

    async fn authorize(
        &self,
        proposal: i64,
        fingerprint: &str,
        kind: &str,
        now: i64,
    ) -> Result<bool> {
        let txn = begin_write(self.db).await?;
        let automatic = if kind == "authorized" {
            " AND approval='automatic'"
        } else {
            ""
        };
        let moved = txn
            .execute_raw(sql(
                &format!("UPDATE optimization_proposals SET state='approved',approved_fingerprint=fingerprint,decided_at=? WHERE workspace_id=? AND id=? AND state='open' AND fingerprint=? AND expires_at>?{automatic} AND (SELECT json_extract(p.policy,'$.mode') FROM optimization_policies p WHERE p.workspace_id=optimization_proposals.workspace_id AND p.ad_account_customer_id=optimization_proposals.ad_account_customer_id ORDER BY p.version DESC LIMIT 1) IN ('recommend','autopilot')"),
                vec![
                    now.into(),
                    self.workspace.id().into(),
                    proposal.into(),
                    fingerprint.into(),
                    now.into(),
                ],
            ))
            .await?
            .rows_affected()
            == 1;
        if moved {
            self.event_in(
                &txn,
                proposal,
                kind,
                &serde_json::json!({"fingerprint": fingerprint}),
                now,
            )
            .await?;
        }
        txn.commit().await?;
        Ok(moved)
    }

    /// Claims an approved Proposal for execution. In the same write transaction it counts the
    /// changes and raises of the day in the Ad Account and refuses while another change of it
    /// runs, so two workers cannot pass a limit together.
    #[allow(clippy::too_many_arguments)]
    pub async fn claim_execution(
        &self,
        proposal: &Proposal,
        day: i64,
        automatic: bool,
        raise_micros: i64,
        changes_per_day: u32,
        max_daily_raise_micros: i64,
        now: i64,
    ) -> Result<Claim> {
        let txn = begin_write(self.db).await?;
        let mut values = self.account_values(&proposal.ad_account);
        let busy = txn
            .query_one_raw(sql(
                &format!("SELECT COUNT(*) AS n FROM optimization_proposals WHERE workspace_id=? AND ad_account_customer_id=? AND state IN ({})", states(&RUNNING)),
                values.clone(),
            ))
            .await?
            .expect("aggregate row")
            .try_get::<i64>("", "n")?;
        if busy > 0 {
            return Ok(Claim::Busy);
        }
        values.push(day.into());
        let today = txn
            .query_one_raw(sql(
                "SELECT COUNT(*) AS n, COALESCE(SUM(e.raise_micros),0) AS raised FROM optimization_executions e JOIN optimization_proposals p ON p.id=e.proposal_id WHERE e.workspace_id=? AND e.ad_account_customer_id=? AND e.day=? AND p.state IN ('applying','applied','uncertain','conflict')",
                values,
            ))
            .await?
            .expect("aggregate row");
        if today.try_get::<i64>("", "n")? >= i64::from(changes_per_day) {
            return Ok(Claim::DailyChanges);
        }
        if raise_micros > 0
            && today.try_get::<i64>("", "raised")? + raise_micros > max_daily_raise_micros
        {
            return Ok(Claim::DailyRaise);
        }
        let claimed = txn
            .execute_raw(sql(
                "UPDATE optimization_proposals SET state='applying' WHERE workspace_id=? AND id=? AND state='approved' AND approved_fingerprint=fingerprint",
                vec![self.workspace.id().into(), proposal.id.into()],
            ))
            .await?
            .rows_affected()
            == 1;
        if !claimed {
            return Ok(Claim::NotApproved);
        }
        txn.execute_raw(sql(
            "INSERT INTO optimization_executions(proposal_id,workspace_id,ad_account_customer_id,day,automatic,raise_micros,started_at) VALUES(?,?,?,?,?,?,?)",
            vec![
                proposal.id.into(),
                self.workspace.id().into(),
                proposal.ad_account.clone().into(),
                day.into(),
                i64::from(automatic).into(),
                raise_micros.into(),
                now.into(),
            ],
        ))
        .await?;
        self.event_in(
            &txn,
            proposal.id,
            "applying",
            &serde_json::json!({"automatic": automatic}),
            now,
        )
        .await?;
        txn.commit().await?;
        Ok(Claim::Claimed)
    }

    pub async fn mark_execution_dispatch(
        &self,
        proposal: i64,
        started_at: i64,
        now: i64,
    ) -> Result<bool> {
        let txn = begin_write(self.db).await?;
        let owned = txn.query_one_raw(sql(
            "SELECT 1 FROM optimization_proposals p JOIN optimization_executions e ON e.proposal_id=p.id AND e.workspace_id=p.workspace_id WHERE p.workspace_id=? AND p.id=? AND p.state='applying' AND p.approved_fingerprint=p.fingerprint AND e.started_at=? AND e.finished_at IS NULL AND NOT EXISTS (SELECT 1 FROM optimization_events WHERE workspace_id=p.workspace_id AND proposal_id=p.id AND kind='dispatching')",
            vec![self.workspace.id().into(), proposal.into(), started_at.into()],
        )).await?.is_some();
        if owned {
            self.event_in(&txn, proposal, "dispatching", &serde_json::json!({}), now)
                .await?;
        }
        txn.commit().await?;
        Ok(owned)
    }

    /// Whether the original reservation still permits its worker to send a change.
    pub async fn execution_is_applying(&self, proposal: i64, started_at: i64) -> Result<bool> {
        Ok(self.db.query_one_raw(sql(
            "SELECT 1 FROM optimization_proposals p JOIN optimization_executions e ON e.proposal_id=p.id AND e.workspace_id=p.workspace_id WHERE p.workspace_id=? AND p.id=? AND p.state='applying' AND p.approved_fingerprint=p.fingerprint AND e.started_at=? AND e.finished_at IS NULL",
            vec![self.workspace.id().into(), proposal.into(), started_at.into()],
        )).await?.is_some())
    }

    /// Records what happened to a running or uncertain change, and moves its Proposal.
    pub async fn finish_execution(
        &self,
        proposal: i64,
        finish: &Finish<'_>,
        now: i64,
    ) -> Result<bool> {
        self.finish(proposal, None, finish, now).await
    }

    /// Records what happened to a change only while the worker that claimed it at `started_at`
    /// still owns it: the Proposal is Applying under that reservation. A reconciliation that
    /// already settled it, or made it Uncertain, wins over a stale worker.
    pub async fn finish_claimed_execution(
        &self,
        proposal: i64,
        started_at: i64,
        finish: &Finish<'_>,
        now: i64,
    ) -> Result<bool> {
        self.finish(proposal, Some(started_at), finish, now).await
    }

    async fn finish(
        &self,
        proposal: i64,
        claimed_at: Option<i64>,
        finish: &Finish<'_>,
        now: i64,
    ) -> Result<bool> {
        let from: &[ProposalState] = if claimed_at.is_some() {
            &[ProposalState::Applying]
        } else {
            &RUNNING
        };
        let allowed = states(
            &from
                .iter()
                .copied()
                .filter(|state| {
                    state.may_become(finish.state)
                        || (*state == ProposalState::Uncertain
                            && finish.state == ProposalState::Uncertain)
                })
                .collect::<Vec<_>>(),
        );
        if allowed.is_empty() {
            return Ok(false);
        }
        let mut values = vec![
            finish.state.name().into(),
            self.workspace.id().into(),
            proposal.into(),
        ];
        let owned = match claimed_at {
            Some(started_at) => {
                values.push(started_at.into());
                " AND EXISTS (SELECT 1 FROM optimization_executions e WHERE e.workspace_id=optimization_proposals.workspace_id AND e.proposal_id=optimization_proposals.id AND e.started_at=?)"
            }
            None => "",
        };
        let txn = begin_write(self.db).await?;
        let account = txn
            .query_one_raw(sql(
                &format!("UPDATE optimization_proposals SET state=? WHERE workspace_id=? AND id=? AND state IN ({allowed}){owned} RETURNING ad_account_customer_id"),
                values,
            ))
            .await?
            .map(|row| row.try_get::<String>("", "ad_account_customer_id"))
            .transpose()?;
        let moved = account.is_some();
        if let Some(account) = account {
            txn.execute_raw(sql(
                "UPDATE optimization_executions SET before=COALESCE(?,before),after=?,request_id=COALESCE(?,request_id),error=?,verified=?,finished_at=? WHERE workspace_id=? AND proposal_id=?",
                vec![
                    finish.before.map(|value| value.to_string()).into(),
                    finish.after.map(|value| value.to_string()).into(),
                    finish.request_id.map(str::to_owned).into(),
                    finish
                        .error
                        .or(finish.refusal.map(Refusal::name))
                        .map(str::to_owned)
                        .into(),
                    i64::from(finish.verified).into(),
                    now.into(),
                    self.workspace.id().into(),
                    proposal.into(),
                ],
            ))
            .await?;
            let mut detail = serde_json::json!({"before": finish.before, "after": finish.after, "request_id": finish.request_id, "error": finish.error, "verified": finish.verified});
            if let Some(refusal) = finish.refusal {
                detail["refusal"] = refusal.name().into();
            }
            self.event_in(&txn, proposal, finish.state.name(), &detail, now)
                .await?;
            if let Some(suspend) = finish.suspend {
                self.suspend_in(&txn, &account, suspend, now).await?;
            }
        }
        txn.commit().await?;
        Ok(moved)
    }

    pub async fn execution(&self, proposal: i64) -> Result<Option<Execution>> {
        self.db
            .query_one_raw(sql(
                "SELECT * FROM optimization_executions WHERE workspace_id=? AND proposal_id=?",
                vec![self.workspace.id().into(), proposal.into()],
            ))
            .await?
            .map(execution)
            .transpose()
    }

    /// Executions of an Ad Account that started at or after a time, newest first.
    pub async fn executions(
        &self,
        id: &str,
        since: i64,
    ) -> Result<Vec<(Execution, ProposalState, Draft)>> {
        let mut values = self.account_values(id);
        values.push(since.into());
        self.db
            .query_all_raw(sql(
                "SELECT e.*,p.state,p.draft FROM optimization_executions e JOIN optimization_proposals p ON p.id=e.proposal_id WHERE e.workspace_id=? AND e.ad_account_customer_id=? AND e.started_at>=? ORDER BY e.started_at DESC",
                values,
            ))
            .await?
            .into_iter()
            .map(|row| {
                let state = ProposalState::from_name(&row.try_get::<String>("", "state")?)
                    .ok_or_else(|| Error::Message("Invalid Proposal state".into()))?;
                let draft = serde_json::from_str(&row.try_get::<String>("", "draft")?)?;
                Ok((execution(row)?, state, draft))
            })
            .collect()
    }

    /// Proposals whose execution did not finish: uncertain ones, and ones still applying
    /// since before a time.
    pub async fn unfinished_executions(&self, started_before: i64) -> Result<Vec<i64>> {
        self.db
            .query_all_raw(sql(
                "SELECT p.id FROM optimization_proposals p JOIN optimization_executions e ON e.proposal_id=p.id WHERE p.workspace_id=? AND (p.state='uncertain' OR (p.state='applying' AND e.started_at<?)) ORDER BY p.id",
                vec![self.workspace.id().into(), started_before.into()],
            ))
            .await?
            .into_iter()
            .map(|row| Ok(row.try_get("", "id")?))
            .collect()
    }

    /// Approved Proposals that no execution claimed, in Ad Accounts where no change runs.
    pub async fn waiting_proposals(&self) -> Result<Vec<i64>> {
        self.db
            .query_all_raw(sql(
                "SELECT p.id FROM optimization_proposals p WHERE p.workspace_id=?1 AND p.state='approved' AND NOT EXISTS (SELECT 1 FROM optimization_executions e WHERE e.proposal_id=p.id) AND NOT EXISTS (SELECT 1 FROM optimization_proposals r WHERE r.workspace_id=?1 AND r.ad_account_customer_id=p.ad_account_customer_id AND r.state IN ('applying','uncertain')) ORDER BY p.id",
                vec![self.workspace.id().into()],
            ))
            .await?
            .into_iter()
            .map(|row| Ok(row.try_get("", "id")?))
            .collect()
    }

    /// Changes of an Ad Account that failed, conflicted or stayed uncertain since a time.
    pub async fn optimization_failures(&self, id: &str, since: i64) -> Result<i64> {
        let mut values = self.account_values(id);
        values.push(since.into());
        Ok(self
            .db
            .query_one_raw(failures(values))
            .await?
            .expect("aggregate row")
            .try_get("", "n")?)
    }

    /// Suspends an active Autopilot inside a transaction, keeping its policy.
    async fn suspend_in(
        &self,
        txn: &impl ConnectionTrait,
        id: &str,
        suspend: Suspend<'_>,
        now: i64,
    ) -> Result<()> {
        let reason = match suspend {
            Suspend::Because(reason) => reason,
            Suspend::AfterFailures { since, limit } => {
                let mut values = self.account_values(id);
                values.push(since.into());
                let failed: i64 = txn
                    .query_one_raw(failures(values))
                    .await?
                    .expect("aggregate row")
                    .try_get("", "n")?;
                if failed < i64::from(limit) {
                    return Ok(());
                }
                "failures"
            }
        };
        let Some(row) = txn
            .query_one_raw(sql(
                "SELECT policy,standing FROM optimization_policies WHERE workspace_id=? AND ad_account_customer_id=? ORDER BY version DESC LIMIT 1",
                self.account_values(id),
            ))
            .await?
        else {
            return Ok(());
        };
        let policy: Policy = serde_json::from_str(&row.try_get::<String>("", "policy")?)?;
        let standing = Standing::from_name(&row.try_get::<String>("", "standing")?);
        if standing == Some(Standing::Active) && policy.mode == Mode::Autopilot {
            self.policy_in(txn, id, &policy, Standing::Suspended, reason, now)
                .await?;
        }
        Ok(())
    }

    /// Adds a line to the history of an Ad Account.
    pub async fn record_optimization_event(
        &self,
        id: &str,
        proposal: Option<i64>,
        kind: &str,
        detail: &serde_json::Value,
        now: i64,
    ) -> Result<()> {
        self.db
            .execute_raw(sql(
                "INSERT INTO optimization_events(workspace_id,ad_account_customer_id,proposal_id,at,kind,detail) SELECT workspace_id,customer_id,?,?,?,? FROM ad_accounts WHERE workspace_id=? AND customer_id=?",
                vec![
                    proposal.into(),
                    now.into(),
                    kind.into(),
                    detail.to_string().into(),
                    self.workspace.id().into(),
                    id.into(),
                ],
            ))
            .await?;
        Ok(())
    }

    /// The history of the Workspace, or of one Ad Account or one Proposal, newest first.
    pub async fn optimization_events(
        &self,
        id: Option<&str>,
        proposal: Option<i64>,
        limit: u32,
    ) -> Result<Vec<Event>> {
        let mut values = vec![self.workspace.id().into()];
        let mut filter = String::new();
        if let Some(id) = id {
            filter.push_str(" AND ad_account_customer_id=?");
            values.push(id.into());
        }
        if let Some(proposal) = proposal {
            filter.push_str(" AND proposal_id=?");
            values.push(proposal.into());
        }
        values.push(limit.into());
        self.db
            .query_all_raw(sql(
                &format!("SELECT * FROM optimization_events WHERE workspace_id=?{filter} ORDER BY at DESC,id DESC LIMIT ?"),
                values,
            ))
            .await?
            .into_iter()
            .map(|row| {
                Ok(Event {
                    id: row.try_get("", "id")?,
                    ad_account: row.try_get("", "ad_account_customer_id")?,
                    proposal_id: row.try_get("", "proposal_id")?,
                    at: row.try_get("", "at")?,
                    kind: row.try_get("", "kind")?,
                    detail: serde_json::from_str(&row.try_get::<String>("", "detail")?)?,
                })
            })
            .collect()
    }

    /// The newest day of data and the time of the last evaluation of an Ad Account.
    pub async fn optimization_evaluation(&self, id: &str) -> Result<Option<(i64, i64)>> {
        self.db
            .query_one_raw(sql(
                "SELECT data_day,evaluated_at FROM optimization_evaluations WHERE workspace_id=? AND ad_account_customer_id=?",
                self.account_values(id),
            ))
            .await?
            .map(|row| Ok((row.try_get("", "data_day")?, row.try_get("", "evaluated_at")?)))
            .transpose()
    }

    pub async fn save_optimization_evaluation(
        &self,
        id: &str,
        data_day: i64,
        signals: i64,
        proposals: i64,
        holds: &serde_json::Value,
        now: i64,
    ) -> Result<()> {
        self.db
            .execute_raw(sql(
                "INSERT INTO optimization_evaluations(workspace_id,ad_account_customer_id,data_day,evaluated_at,signals,proposals,holds) SELECT workspace_id,customer_id,?,?,?,?,? FROM ad_accounts WHERE workspace_id=? AND customer_id=? ON CONFLICT(workspace_id,ad_account_customer_id) DO UPDATE SET data_day=excluded.data_day,evaluated_at=excluded.evaluated_at,signals=excluded.signals,proposals=excluded.proposals,holds=excluded.holds",
                vec![
                    data_day.into(),
                    now.into(),
                    signals.into(),
                    proposals.into(),
                    holds.to_string().into(),
                    self.workspace.id().into(),
                    id.into(),
                ],
            ))
            .await?;
        Ok(())
    }

    /// The holds of the last evaluation, by reason.
    pub async fn optimization_holds(&self, id: &str) -> Result<serde_json::Value> {
        Ok(self
            .db
            .query_one_raw(sql(
                "SELECT holds FROM optimization_evaluations WHERE workspace_id=? AND ad_account_customer_id=?",
                self.account_values(id),
            ))
            .await?
            .map(|row| row.try_get::<String>("", "holds"))
            .transpose()?
            .map(|holds| serde_json::from_str(&holds))
            .transpose()?
            .unwrap_or(serde_json::Value::Null))
    }
}

fn execution(row: QueryResult) -> Result<Execution> {
    let json = |column: &str| -> Result<Option<serde_json::Value>> {
        row.try_get::<Option<String>>("", column)?
            .map(|text| serde_json::from_str(&text))
            .transpose()
            .map_err(Into::into)
    };
    Ok(Execution {
        proposal_id: row.try_get("", "proposal_id")?,
        ad_account: row.try_get("", "ad_account_customer_id")?,
        day: row.try_get("", "day")?,
        automatic: row.try_get::<i64>("", "automatic")? == 1,
        before: json("before")?,
        after: json("after")?,
        request_id: row.try_get("", "request_id")?,
        error: row.try_get("", "error")?,
        verified: row.try_get::<i64>("", "verified")? == 1,
        started_at: row.try_get("", "started_at")?,
        finished_at: row.try_get("", "finished_at")?,
    })
}
