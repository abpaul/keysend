use std::collections::BTreeMap;

use crate::monitoring::metrics::Metrics;
use loco_rs::Result;
use sea_orm::ConnectionTrait;

use super::{sql, WorkspaceStore};

#[derive(Clone, Debug)]
pub struct DeviceTotal {
    pub device: i64,
    pub spend: f64,
    pub clicks: f64,
    pub conversions: f64,
}

impl WorkspaceStore<'_> {
    pub async fn device_totals(
        &self,
        id: &str,
        start: i64,
        end: i64,
        campaign: Option<i64>,
    ) -> Result<Vec<DeviceTotal>> {
        Ok(self
            .device_metrics(id, start, end, campaign)
            .await?
            .into_iter()
            .map(|(device, metrics)| DeviceTotal {
                device,
                spend: metrics.spend,
                clicks: metrics.clicks,
                conversions: metrics.conversions.unwrap_or_default(),
            })
            .collect())
    }

    pub async fn device_metrics(
        &self,
        id: &str,
        start: i64,
        end: i64,
        campaign: Option<i64>,
    ) -> Result<BTreeMap<i64, Metrics>> {
        let mut values = self.monitoring_account(id);
        values.extend([start.into(), end.into(), campaign.into()]);
        self.db.query_all_raw(sql(
            "SELECT device,SUM(cost_micros) AS cost,SUM(clicks) AS clicks,SUM(impressions) AS impressions,SUM(conversions) AS conversions,SUM(conversions_value) AS conversion_value FROM campaign_days WHERE workspace_id=?1 AND ad_account_customer_id=?2 AND day BETWEEN ?3 AND ?4 AND (?5 IS NULL OR campaign_id=?5) GROUP BY device ORDER BY device", values,
        )).await?.into_iter().map(|row| Ok((row.try_get("", "device")?, Metrics {
            spend: row.try_get::<i64>("", "cost")? as f64 / 1_000_000.0,
            clicks: row.try_get::<i64>("", "clicks")? as f64,
            impressions: row.try_get::<i64>("", "impressions")? as f64,
            conversions: Some(row.try_get("", "conversions")?),
            conversion_value: Some(row.try_get("", "conversion_value")?),
        }))).collect()
    }
}
