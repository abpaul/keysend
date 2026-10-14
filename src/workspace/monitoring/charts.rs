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
        let mut values = self.monitoring_account(id);
        values.extend([start.into(), end.into(), campaign.into()]);
        self.db
            .query_all_raw(sql(
                "SELECT device,SUM(cost_micros) AS cost,SUM(clicks) AS clicks,SUM(conversions) AS conversions FROM campaign_days WHERE workspace_id=?1 AND ad_account_customer_id=?2 AND day BETWEEN ?3 AND ?4 AND (?5 IS NULL OR campaign_id=?5) GROUP BY device ORDER BY device",
                values,
            ))
            .await?
            .into_iter()
            .map(|row| {
                Ok(DeviceTotal {
                    device: row.try_get("", "device")?,
                    spend: row.try_get::<i64>("", "cost")? as f64 / 1_000_000.0,
                    clicks: row.try_get::<i64>("", "clicks")? as f64,
                    conversions: row.try_get("", "conversions")?,
                })
            })
            .collect()
    }
}
