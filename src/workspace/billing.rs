use loco_rs::Result;
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement, TransactionTrait, Value};

use crate::billing::{Checkout, Subscription, VerifiedEvent};

use super::WorkspaceStore;

fn sql(query: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Sqlite, query, values)
}

impl WorkspaceStore<'_> {
    pub async fn stripe_customer(&self) -> Result<Option<String>> {
        self.db
            .query_one_raw(sql(
                "SELECT stripe_customer_id FROM stripe_customers WHERE workspace_id=?",
                vec![self.workspace.id().into()],
            ))
            .await?
            .map(|row| Ok(row.try_get("", "stripe_customer_id")?))
            .transpose()
    }

    pub async fn save_stripe_customer(&self, id: &str) -> Result<()> {
        self.db.execute_raw(sql("INSERT INTO stripe_customers(workspace_id,stripe_customer_id) VALUES (?,?) ON CONFLICT(workspace_id) DO NOTHING",vec![self.workspace.id().into(),id.into()])).await?;
        Ok(())
    }

    pub async fn save_checkout(&self, checkout: &Checkout) -> Result<()> {
        self.db.execute_raw(sql("INSERT INTO billing_checkouts(workspace_id,id,amount,created_at) VALUES (?,?,?,?) ON CONFLICT(workspace_id,id) DO NOTHING",vec![self.workspace.id().into(),checkout.id.clone().into(),checkout.amount.into(),crate::settings::wall_clock().timestamp().into()])).await?;
        Ok(())
    }

    pub async fn checkout(&self, id: &str) -> Result<Option<Checkout>> {
        self.db
            .query_one_raw(sql(
                "SELECT id,amount FROM billing_checkouts WHERE workspace_id=? AND id=?",
                vec![self.workspace.id().into(), id.into()],
            ))
            .await?
            .map(|row| {
                Ok(Checkout {
                    id: row.try_get("", "id")?,
                    amount: row.try_get("", "amount")?,
                })
            })
            .transpose()
    }

    pub async fn subscription(&self) -> Result<Option<Subscription>> {
        self.db.query_one_raw(sql("SELECT stripe_subscription_id,created_at,status,period_start,period_end,cancel_at_period_end,canceled_at FROM subscriptions WHERE workspace_id=?",vec![self.workspace.id().into()])).await?.map(|row| Ok(Subscription{id:row.try_get("","stripe_subscription_id")?,created_at:row.try_get("","created_at")?,status:row.try_get::<String>("","status")?.parse().map_err(|()| loco_rs::Error::Message("Unknown subscription status".into()))?,period_start:row.try_get("","period_start")?,period_end:row.try_get("","period_end")?,cancel_at_period_end:row.try_get("","cancel_at_period_end")?,canceled_at:row.try_get("","canceled_at")?})).transpose()
    }

    pub async fn subscription_active(&self) -> Result<bool> {
        Ok(self
            .db
            .query_one_raw(sql(
                "SELECT 1 FROM subscriptions WHERE workspace_id=? AND status='active' AND period_end>?",
                vec![self.workspace.id().into(), crate::settings::wall_clock().timestamp().into()],
            ))
            .await?
            .is_some())
    }

    pub async fn apply_billing_event(
        &self,
        event: &VerifiedEvent,
        checkout: Option<&Checkout>,
        subscription: Option<&Subscription>,
    ) -> Result<()> {
        let txn = self.db.begin().await?;
        let inserted=txn.execute_raw(sql("INSERT INTO stripe_events(workspace_id,id) VALUES (?,?) ON CONFLICT(workspace_id,id) DO NOTHING",vec![self.workspace.id().into(),event.id().into()])).await?;
        if inserted.rows_affected() == 0 {
            txn.commit().await?;
            return Ok(());
        }
        if let Some(checkout) = checkout {
            let paid_at = event.created();
            txn.execute_raw(sql("UPDATE billing_checkouts SET paid_at=COALESCE(paid_at,?) WHERE workspace_id=? AND id=?",vec![paid_at.into(),self.workspace.id().into(),checkout.id.clone().into()])).await?;
        }
        if let Some(subscription) = subscription {
            // Order observations within the same second so a cancellation toggle cannot collide.
            let changes = txn.execute_raw(sql(
                "INSERT INTO subscription_changes(workspace_id,observed_at,status,cancel_at_period_end) SELECT ?1,MAX(?2,COALESCE((SELECT MAX(observed_at)+1 FROM subscription_changes WHERE workspace_id=?1),?2)),?3,?4 WHERE NOT EXISTS(SELECT 1 FROM subscriptions WHERE workspace_id=?1 AND (created_at>?5 OR (status=?3 AND cancel_at_period_end=?4)))",
                vec![self.workspace.id().into(), crate::settings::wall_clock().timestamp().into(), subscription.status.as_str().into(), subscription.cancel_at_period_end.into(), subscription.created_at.into()],
            )).await.map(|_| ()).map_err(Into::into);
            super::measurement::recorded(changes);
            txn.execute_raw(sql("INSERT INTO subscriptions(workspace_id,stripe_subscription_id,created_at,status,period_start,period_end,cancel_at_period_end,canceled_at) VALUES (?,?,?,?,?,?,?,?) ON CONFLICT(workspace_id) DO UPDATE SET stripe_subscription_id=excluded.stripe_subscription_id,created_at=excluded.created_at,status=excluded.status,period_start=excluded.period_start,period_end=excluded.period_end,cancel_at_period_end=excluded.cancel_at_period_end,canceled_at=excluded.canceled_at WHERE excluded.created_at>=subscriptions.created_at",vec![self.workspace.id().into(),subscription.id.clone().into(),subscription.created_at.into(),subscription.status.as_str().into(),subscription.period_start.into(),subscription.period_end.into(),subscription.cancel_at_period_end.into(),subscription.canceled_at.into()])).await?;
        }
        txn.commit().await?;
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct SubscriptionChange {
    pub observed_at: i64,
    pub status: crate::billing::SubscriptionStatus,
    pub cancel_at_period_end: bool,
}

impl WorkspaceStore<'_> {
    pub async fn subscription_changes(&self) -> Result<Vec<SubscriptionChange>> {
        self.db.query_all_raw(sql("SELECT observed_at,status,cancel_at_period_end FROM subscription_changes WHERE workspace_id=? ORDER BY observed_at,status", vec![self.workspace.id().into()])).await?.into_iter().map(|row| Ok(SubscriptionChange {
            observed_at: row.try_get("", "observed_at")?,
            status: row.try_get::<String>("", "status")?.parse().map_err(|()| loco_rs::Error::Message("Unknown subscription status".into()))?,
            cancel_at_period_end: row.try_get("", "cancel_at_period_end")?,
        })).collect()
    }
}
