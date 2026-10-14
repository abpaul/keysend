use loco_rs::Result;
use sea_orm::{ConnectionTrait, TransactionTrait};

use super::{sql, WorkspaceStore};
use crate::monitoring::{
    alerts::Alert,
    email::{self, WeeklyEmail},
};

impl WorkspaceStore<'_> {
    pub async fn save_alerts(&self, id: &str, alerts: &[Alert], now: i64) -> Result<()> {
        self.save_alerts_on(self.db, id, alerts, now).await
    }

    async fn save_alerts_on(
        &self,
        db: &impl ConnectionTrait,
        id: &str,
        alerts: &[Alert],
        now: i64,
    ) -> Result<()> {
        for alert in alerts {
            let mut values = self.monitoring_account(id);
            values.extend([
                alert.day.into(),
                alert.event_key.to_string().into(),
                serde_json::to_string(alert)?.into(),
                alert.body.clone().into(),
                now.into(),
            ]);
            db.execute_raw(sql("INSERT INTO alerts(workspace_id,ad_account_customer_id,day,event_key,data,body,created_at) SELECT workspace_id,customer_id,?3,?4,?5,?6,?7 FROM ad_accounts WHERE workspace_id=?1 AND customer_id=?2 AND NOT EXISTS(SELECT 1 FROM alerts WHERE workspace_id=?1 AND ad_account_customer_id=?2 AND day BETWEEN ?3-6 AND ?3+6 AND event_key=?4) ON CONFLICT(workspace_id,ad_account_customer_id,day,event_key) DO NOTHING",values)).await?;
        }
        Ok(())
    }

    pub async fn alert_evaluated(&self, id: &str, day: i64) -> Result<bool> {
        let mut values = self.monitoring_account(id);
        values.push(day.into());
        Ok(self.db.query_one_raw(sql("SELECT 1 FROM alert_evaluations WHERE workspace_id=? AND ad_account_customer_id=? AND day=?",values)).await?.is_some())
    }

    pub async fn evaluate_alerts(
        &self,
        id: &str,
        day: i64,
        alerts: &[Alert],
        now: i64,
    ) -> Result<()> {
        let txn = self.db.begin().await?;
        self.save_alerts_on(&txn, id, alerts, now).await?;
        let mut values = self.monitoring_account(id);
        values.push(day.into());
        txn.execute_raw(sql("INSERT INTO alert_evaluations(workspace_id,ad_account_customer_id,day) VALUES(?,?,?) ON CONFLICT DO NOTHING",values)).await?;
        txn.commit().await?;
        Ok(())
    }

    pub async fn abandon_unconfirmed_alerts(&self, id: &str, now: i64) -> Result<Vec<Alert>> {
        let mut values = self.monitoring_account(id);
        values.push(now.into());
        self.db.query_all_raw(sql("UPDATE alerts SET cancelled_at=?3,claimed_at=NULL WHERE workspace_id=?1 AND ad_account_customer_id=?2 AND sent_at IS NULL AND cancelled_at IS NULL AND claimed_at<?3-600 RETURNING data",values)).await?.into_iter().map(|row|Ok(serde_json::from_str(&row.try_get::<String>("","data")?)?)).collect()
    }

    pub async fn expire_alerts(&self, id: &str, now: i64) -> Result<()> {
        let mut values = self.monitoring_account(id);
        values.push(now.into());
        self.db.execute_raw(sql("UPDATE alerts SET cancelled_at=?3,claimed_at=NULL WHERE workspace_id=?1 AND ad_account_customer_id=?2 AND sent_at IS NULL AND cancelled_at IS NULL AND created_at<?3-172800 AND claimed_at IS NULL",values)).await?;
        Ok(())
    }

    pub async fn alerts(&self, id: &str, start: i64, end: i64) -> Result<Vec<Alert>> {
        let mut values = self.monitoring_account(id);
        values.extend([start.into(), end.into()]);
        self.db.query_all_raw(sql("SELECT data FROM alerts WHERE workspace_id=? AND ad_account_customer_id=? AND day BETWEEN ? AND ? ORDER BY day,event_key",values)).await?.into_iter().map(|row|Ok(serde_json::from_str(&row.try_get::<String>("","data")?)?)).collect()
    }

    pub async fn pending_alerts(&self, id: &str) -> Result<Vec<Alert>> {
        self.db.query_all_raw(sql("SELECT data FROM alerts WHERE workspace_id=? AND ad_account_customer_id=? AND sent_at IS NULL AND cancelled_at IS NULL ORDER BY day,event_key",self.monitoring_account(id))).await?.into_iter().map(|row|Ok(serde_json::from_str(&row.try_get::<String>("","data")?)?)).collect()
    }

    pub async fn claim_alert(&self, id: &str, alert: &Alert, now: i64) -> Result<bool> {
        let mut values = vec![now.into()];
        values.extend(self.monitoring_account(id));
        values.extend([alert.day.into(), alert.event_key.to_string().into()]);
        Ok(self.db.execute_raw(sql("UPDATE alerts SET claimed_at=?1 WHERE workspace_id=?2 AND ad_account_customer_id=?3 AND day=?4 AND event_key=?5 AND sent_at IS NULL AND cancelled_at IS NULL AND created_at>=?1-172800 AND claimed_at IS NULL AND NOT EXISTS(SELECT 1 FROM alerts prior WHERE prior.workspace_id=?2 AND prior.ad_account_customer_id=?3 AND prior.sent_at>?1-604800 AND prior.event_key=?5)",values)).await?.rows_affected()==1)
    }

    pub async fn finish_alert_email(
        &self,
        id: &str,
        alert: &Alert,
        now: i64,
        sent: bool,
    ) -> Result<()> {
        let mut values = vec![if sent { Some(now) } else { None }.into()];
        values.extend(self.monitoring_account(id));
        values.extend([
            alert.day.into(),
            alert.event_key.to_string().into(),
            now.into(),
        ]);
        self.db.execute_raw(sql("UPDATE alerts SET sent_at=?,claimed_at=NULL WHERE workspace_id=? AND ad_account_customer_id=? AND day=? AND event_key=? AND claimed_at=?",values)).await?;
        Ok(())
    }

    pub async fn save_weekly_email(&self, email: &WeeklyEmail, now: i64) -> Result<()> {
        self.db.execute_raw(sql("INSERT INTO weekly_emails(workspace_id,day,data,body,created_at) VALUES(?,?,?,?,?) ON CONFLICT(workspace_id,day) DO NOTHING",vec![self.workspace.id().into(),email.day.into(),serde_json::to_string(email)?.into(),email.body.clone().into(),now.into()])).await?;
        Ok(())
    }

    pub async fn weekly_email(&self, day: i64) -> Result<Option<WeeklyEmail>> {
        self.db
            .query_one_raw(sql(
                "SELECT data FROM weekly_emails WHERE workspace_id=? AND day=?",
                vec![self.workspace.id().into(), day.into()],
            ))
            .await?
            .map(|row| Ok(serde_json::from_str(&row.try_get::<String>("", "data")?)?))
            .transpose()
    }

    pub async fn abandon_unconfirmed_weekly_emails(&self, now: i64) -> Result<Vec<WeeklyEmail>> {
        self.db.query_all_raw(sql("UPDATE weekly_emails SET cancelled_at=?1,claimed_at=NULL WHERE workspace_id=?2 AND sent_at IS NULL AND cancelled_at IS NULL AND claimed_at<?1-600 RETURNING data",vec![now.into(),self.workspace.id().into()])).await?.into_iter().map(|row|Ok(serde_json::from_str(&row.try_get::<String>("","data")?)?)).collect()
    }

    pub async fn expire_weekly_emails(&self, now: i64) -> Result<()> {
        self.db.execute_raw(sql("UPDATE weekly_emails SET cancelled_at=?1,claimed_at=NULL WHERE workspace_id=?2 AND sent_at IS NULL AND cancelled_at IS NULL AND created_at<?1-172800 AND claimed_at IS NULL",vec![now.into(),self.workspace.id().into()])).await?;
        Ok(())
    }

    pub async fn pending_weekly_emails(&self) -> Result<Vec<WeeklyEmail>> {
        self.db.query_all_raw(sql("SELECT data FROM weekly_emails WHERE workspace_id=? AND sent_at IS NULL AND cancelled_at IS NULL ORDER BY day",vec![self.workspace.id().into()])).await?.into_iter().map(|row|Ok(serde_json::from_str(&row.try_get::<String>("","data")?)?)).collect()
    }

    pub async fn claim_weekly_email(&self, day: i64, now: i64) -> Result<bool> {
        Ok(self.db.execute_raw(sql("UPDATE weekly_emails SET claimed_at=?1 WHERE workspace_id=?2 AND day=?3 AND sent_at IS NULL AND cancelled_at IS NULL AND created_at>=?1-172800 AND claimed_at IS NULL",vec![now.into(),self.workspace.id().into(),day.into()])).await?.rows_affected()==1)
    }

    pub async fn prepare_weekly_delivery(
        &self,
        pending: &WeeklyEmail,
        now: i64,
    ) -> Result<Option<WeeklyEmail>> {
        let accounts = self.ad_accounts().await?;
        let mut blocks = Vec::new();
        for block in &pending.blocks {
            if accounts.iter().any(|ad| ad.customer_id == block.ad_account)
                && self
                    .monitoring_settings(&block.ad_account)
                    .await?
                    .weekly_email
            {
                blocks.push(block.clone());
            }
        }
        if blocks.is_empty() {
            self.db.execute_raw(sql("UPDATE weekly_emails SET cancelled_at=?1,claimed_at=NULL WHERE workspace_id=?2 AND day=?3 AND claimed_at=?1 AND sent_at IS NULL",vec![now.into(),self.workspace.id().into(),pending.day.into()])).await?;
            return Ok(None);
        }
        let left_out = pending.left_out.clone();
        let content = email::content(pending.day, blocks, left_out)?;
        self.db.execute_raw(sql("UPDATE weekly_emails SET data=?1,body=?2 WHERE workspace_id=?3 AND day=?4 AND claimed_at=?5 AND sent_at IS NULL",vec![serde_json::to_string(&content)?.into(),content.body.clone().into(),self.workspace.id().into(),content.day.into(),now.into()])).await?;
        Ok(Some(content))
    }

    pub async fn finish_weekly_email(&self, day: i64, now: i64, sent: bool) -> Result<()> {
        self.db.execute_raw(sql("UPDATE weekly_emails SET sent_at=?,claimed_at=NULL WHERE workspace_id=? AND day=? AND claimed_at=?",vec![if sent {Some(now)} else {None}.into(),self.workspace.id().into(),day.into(),now.into()])).await?;
        Ok(())
    }
}
