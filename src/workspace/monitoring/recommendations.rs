use loco_rs::Result;
use sea_orm::ConnectionTrait;
use serde::{Deserialize, Serialize};

use super::{sql, WorkspaceStore};
use crate::database::begin_write;
use crate::monitoring::{
    campaign_text::without_campaign_id,
    deep_check::{DeepCheck, Finding},
    earlier::Earlier,
    recommendation::Confidence,
};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecommendationState {
    Open,
    Done,
    NotUseful,
    Resolved,
}

impl RecommendationState {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Done => "done",
            Self::NotUseful => "not_useful",
            Self::Resolved => "resolved",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Recommendation {
    pub finding_key: String,
    pub action: String,
    pub reason: String,
    pub evidence: String,
    #[serde(default)]
    pub measure: String,
    /// What the action does, from `recommendation::INTENTS`.
    #[serde(default)]
    pub intent: String,
    /// The campaign, search term, page or location that the action names.
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub confidence: Confidence,
    pub campaign_id: Option<i64>,
    pub state: RecommendationState,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Detected {
    pub title: String,
    pub spend: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RecommendationWithFinding {
    pub recommendation: Recommendation,
    pub finding: Option<Detected>,
}

impl Finding {
    fn name_only(mut self) -> Self {
        if let Some(campaign) = self.campaign_id {
            let rule = &mut self.rule;
            for text in [&mut rule.title, &mut rule.detail]
                .into_iter()
                .chain(&mut rule.subjects)
            {
                *text = without_campaign_id(text, campaign);
            }
        }
        self
    }
}

impl Recommendation {
    fn name_only(mut self) -> Self {
        if let Some(campaign) = self.campaign_id {
            for text in [&mut self.action, &mut self.reason, &mut self.evidence] {
                *text = without_campaign_id(text, campaign);
            }
        }
        self
    }
}

impl WorkspaceStore<'_> {
    pub async fn monitoring_findings(&self, id: &str) -> Result<Vec<Finding>> {
        self.db.query_all_raw(sql("SELECT data FROM monitoring_findings WHERE workspace_id=? AND ad_account_customer_id=? AND active=1 ORDER BY finding_key",self.monitoring_account(id))).await?.into_iter().map(|row|Ok(serde_json::from_str::<Finding>(&row.try_get::<String>("","data")?)?.name_only())).collect()
    }

    /// Active and resolved Findings, with the state recorded by the latest Deep check.
    pub async fn monitoring_finding_history(&self, id: &str) -> Result<Vec<(Finding, bool)>> {
        self.db.query_all_raw(sql("SELECT data,active FROM monitoring_findings WHERE workspace_id=? AND ad_account_customer_id=? ORDER BY finding_key", self.monitoring_account(id))).await?.into_iter().map(|row| Ok((serde_json::from_str::<Finding>(&row.try_get::<String>("", "data")?)?.name_only(), row.try_get::<i64>("", "active")? == 1))).collect()
    }

    pub async fn monitoring_recommendations(&self, id: &str) -> Result<Vec<Recommendation>> {
        self.db.query_all_raw(sql("SELECT data,state FROM monitoring_recommendations WHERE workspace_id=? AND ad_account_customer_id=? ORDER BY opened_day,finding_key",self.monitoring_account(id))).await?.into_iter().map(|row| {
            let mut recommendation:Recommendation=serde_json::from_str(&row.try_get::<String>("","data")?)?;
            recommendation.state=serde_json::from_value(serde_json::Value::String(row.try_get("","state")?))?;Ok(recommendation.name_only())
        }).collect()
    }

    /// Every Recommendation of the Ad Account with the days it opened and closed and whether
    /// its Finding is still detected.
    pub async fn recommendation_history(&self, id: &str) -> Result<Vec<Earlier>> {
        self.db.query_all_raw(sql("SELECT r.data,r.state,r.opened_day,r.closed_day,EXISTS(SELECT 1 FROM monitoring_findings f WHERE f.workspace_id=r.workspace_id AND f.ad_account_customer_id=r.ad_account_customer_id AND f.finding_key=r.finding_key AND f.active=1) AS detected FROM monitoring_recommendations r WHERE r.workspace_id=? AND r.ad_account_customer_id=? ORDER BY r.opened_day,r.finding_key",self.monitoring_account(id))).await?.into_iter().map(|row| {
            let mut recommendation:Recommendation=serde_json::from_str(&row.try_get::<String>("","data")?)?;
            recommendation.state=serde_json::from_value(serde_json::Value::String(row.try_get("","state")?))?;
            Ok(Earlier{recommendation:recommendation.name_only(),opened_day:row.try_get("","opened_day")?,closed_day:row.try_get("","closed_day")?,still_detected:row.try_get::<i64>("","detected")?==1})
        }).collect()
    }

    pub async fn recommendations_with_findings(
        &self,
        id: &str,
    ) -> Result<Vec<RecommendationWithFinding>> {
        self.db.query_all_raw(sql("SELECT r.data,r.state,json_extract(f.data,'$.title') AS title,json_extract(f.data,'$.spend') AS spend FROM monitoring_recommendations r LEFT JOIN monitoring_findings f ON f.workspace_id=r.workspace_id AND f.ad_account_customer_id=r.ad_account_customer_id AND f.finding_key=r.finding_key AND f.active=1 WHERE r.workspace_id=? AND r.ad_account_customer_id=? ORDER BY r.opened_day,r.finding_key",self.monitoring_account(id))).await?.into_iter().map(|row| {
            let mut recommendation:Recommendation=serde_json::from_str(&row.try_get::<String>("","data")?)?;
            recommendation.state=serde_json::from_value(serde_json::Value::String(row.try_get("","state")?))?;
            let recommendation=recommendation.name_only();
            let title=row.try_get::<Option<String>>("","title")?;
            let spend=row.try_get::<Option<f64>>("","spend")?;
            let finding=title.zip(spend).map(|(title,spend)|Detected{title:match recommendation.campaign_id{Some(campaign)=>without_campaign_id(&title,campaign),None=>title},spend});
            Ok(RecommendationWithFinding{recommendation,finding})
        }).collect()
    }

    pub async fn close_recommendation(
        &self,
        id: &str,
        key: &str,
        state: RecommendationState,
        day: i64,
    ) -> Result<bool> {
        if !matches!(
            state,
            RecommendationState::Done | RecommendationState::NotUseful
        ) {
            return Ok(false);
        }
        let mut values = vec![state.as_str().into(), day.into()];
        values.extend(self.monitoring_account(id));
        values.push(key.into());
        Ok(self.db.execute_raw(sql("UPDATE monitoring_recommendations SET state=?,closed_day=? WHERE workspace_id=? AND ad_account_customer_id=? AND finding_key=? AND state='open'",values)).await?.rows_affected()==1)
    }

    pub async fn recommendation_blocked_keys(
        &self,
        id: &str,
        day: i64,
    ) -> Result<std::collections::BTreeSet<String>> {
        let mut values = self.monitoring_account(id);
        values.push(day.into());
        self.db.query_all_raw(sql("SELECT finding_key FROM monitoring_recommendations WHERE workspace_id=? AND ad_account_customer_id=? AND (state='open' OR (state IN ('done','not_useful') AND closed_day>?-90))",values)).await?.into_iter().map(|row|row.try_get("","finding_key").map_err(Into::into)).collect()
    }

    pub async fn deep_check_failed(&self, id: &str, day: i64, error: &str) -> Result<()> {
        let mut values = self.monitoring_account(id);
        values.extend([day.into(), error.into()]);
        self.db.execute_raw(sql("INSERT INTO deep_check_failures(workspace_id,ad_account_customer_id,day,error) VALUES(?,?,?,?) ON CONFLICT(workspace_id,ad_account_customer_id) DO UPDATE SET day=excluded.day,error=excluded.error",values)).await?;
        Ok(())
    }

    pub async fn refresh_deep_check(
        &self,
        id: &str,
        check: &DeepCheck,
        recommendations: &[Recommendation],
    ) -> Result<()> {
        let txn = begin_write(self.db).await?;
        let Some(_) = txn
            .query_one_raw(sql(
                "SELECT 1 FROM ad_accounts WHERE workspace_id=? AND customer_id=?",
                self.monitoring_account(id),
            ))
            .await?
        else {
            return Ok(());
        };
        let existing = txn
            .query_one_raw(sql(
                "SELECT day FROM deep_checks WHERE workspace_id=? AND ad_account_customer_id=?",
                self.monitoring_account(id),
            ))
            .await?;
        if existing.is_some_and(|row| {
            row.try_get::<i64>("", "day")
                .is_ok_and(|day| day > check.day)
        }) {
            return Ok(());
        }
        let mut resolved = self.monitoring_account(id);
        resolved.push(check.day.into());
        txn.execute_raw(sql("UPDATE monitoring_findings SET active=0,last_day=?3 WHERE workspace_id=?1 AND ad_account_customer_id=?2",resolved)).await?;
        for finding in &check.findings {
            let mut values = self.monitoring_account(id);
            values.extend([
                finding.rule.key.clone().into(),
                finding.campaign_id.into(),
                serde_json::to_string(finding)?.into(),
                check.day.into(),
            ]);
            txn.execute_raw(sql("INSERT INTO monitoring_findings(workspace_id,ad_account_customer_id,finding_key,campaign_id,data,active,first_day,last_day) VALUES(?1,?2,?3,?4,?5,1,?6,?6) ON CONFLICT(workspace_id,ad_account_customer_id,finding_key) DO UPDATE SET data=excluded.data,campaign_id=excluded.campaign_id,active=1,last_day=excluded.last_day",values)).await?;
        }
        txn.execute_raw(sql("UPDATE monitoring_recommendations SET state='resolved' WHERE state='open' AND workspace_id=? AND ad_account_customer_id=? AND EXISTS(SELECT 1 FROM monitoring_findings f WHERE f.workspace_id=monitoring_recommendations.workspace_id AND f.ad_account_customer_id=monitoring_recommendations.ad_account_customer_id AND f.finding_key=monitoring_recommendations.finding_key AND f.active=0)",self.monitoring_account(id))).await?;
        for recommendation in recommendations {
            if !check
                .findings
                .iter()
                .any(|finding| finding.rule.key == recommendation.finding_key)
            {
                continue;
            }
            let mut values = self.monitoring_account(id);
            values.extend([
                recommendation.finding_key.clone().into(),
                serde_json::to_string(recommendation)?.into(),
                check.day.into(),
            ]);
            txn.execute_raw(sql("INSERT INTO monitoring_recommendations(workspace_id,ad_account_customer_id,finding_key,data,state,opened_day) VALUES(?,?,?,?,'open',?) ON CONFLICT(workspace_id,ad_account_customer_id,finding_key) DO UPDATE SET data=excluded.data,state='open',opened_day=excluded.opened_day,closed_day=NULL WHERE state='resolved' OR (state IN ('done','not_useful') AND closed_day<=excluded.opened_day-90)",values)).await?;
        }
        let mut values = self.monitoring_account(id);
        values.extend([
            check.day.into(),
            serde_json::to_string(check)?.into(),
            check.data.audit.conversion_actions.into(),
        ]);
        txn.execute_raw(sql("INSERT INTO deep_checks(workspace_id,ad_account_customer_id,day,data,conversion_actions) VALUES(?,?,?,?,?) ON CONFLICT(workspace_id,ad_account_customer_id) DO UPDATE SET day=excluded.day,data=excluded.data,conversion_actions=excluded.conversion_actions",values)).await?;
        txn.execute_raw(sql(
            "DELETE FROM deep_check_failures WHERE workspace_id=? AND ad_account_customer_id=?",
            self.monitoring_account(id),
        ))
        .await?;
        txn.commit().await?;
        if check.assessment.is_some() {
            self.record_activity(crate::workspace::measurement::Activity::Assessed)
                .await;
        }
        Ok(())
    }
}
