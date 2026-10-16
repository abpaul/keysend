use std::collections::BTreeMap;

use loco_rs::Result;
use sea_orm::{ConnectionTrait, Value};

use super::{sql, WorkspaceStore};
use crate::{database::begin_write, monitoring::audit::Counts};

const BATCH: usize = 90;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Breakdown {
    SearchShare,
    ConversionActions,
    Networks,
    Hours,
    Locations,
    KeywordQuality,
    Age,
    Gender,
    ConversionDelay,
}

impl Breakdown {
    pub fn key(self) -> &'static str {
        match self {
            Self::SearchShare => "search_share",
            Self::ConversionActions => "conversion_actions",
            Self::Networks => "networks",
            Self::Hours => "hours",
            Self::Locations => "locations",
            Self::KeywordQuality => "keyword_quality",
            Self::Age => "age",
            Self::Gender => "gender",
            Self::ConversionDelay => "conversion_delay",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BreakdownState {
    pub first_day: Option<i64>,
    pub last_day: Option<i64>,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Share {
    Exact(f64),
    OutsideExactRange,
    Unknown,
}

impl Share {
    fn columns(self) -> [Value; 2] {
        match self {
            Self::Exact(value) => [value.into(), 0.into()],
            Self::OutsideExactRange => [Option::<f64>::None.into(), 1.into()],
            Self::Unknown => [Option::<f64>::None.into(), 0.into()],
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchShareDay {
    pub day: i64,
    pub campaign_id: i64,
    pub impressions: i64,
    pub shown: Share,
    pub lost_budget: Share,
    pub lost_rank: Share,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ActionDay {
    pub day: i64,
    pub campaign_id: i64,
    pub action: String,
    pub conversions: f64,
    pub value: f64,
}

#[derive(Clone, Debug)]
pub struct NetworkDay {
    pub day: i64,
    pub campaign_id: i64,
    pub network: String,
    pub counts: Counts,
}

pub enum BreakdownDays {
    SearchShare(Vec<SearchShareDay>),
    ConversionActions(Vec<ActionDay>),
    Networks(Vec<NetworkDay>),
}

impl BreakdownDays {
    fn kind(&self) -> Breakdown {
        match self {
            Self::SearchShare(_) => Breakdown::SearchShare,
            Self::ConversionActions(_) => Breakdown::ConversionActions,
            Self::Networks(_) => Breakdown::Networks,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchShareTotal {
    pub campaign_id: i64,
    pub shares: Option<SearchShares>,
    pub limit_days: i64,
    pub exact_days: i64,
    pub days: i64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SearchShares {
    pub shown: f64,
    pub lost_budget: f64,
    pub lost_rank: f64,
}

pub struct NetworkTotal {
    pub network: String,
    pub spend: f64,
    pub clicks: f64,
    pub conversions: f64,
}

pub(super) fn counts_values(counts: &Counts) -> [Value; 5] {
    [
        (counts.impressions.round() as i64).into(),
        (counts.clicks.round() as i64).into(),
        ((counts.cost * 1_000_000.0).round() as i64).into(),
        counts.conversions.into(),
        counts.value.into(),
    ]
}

pub(super) async fn insert(
    db: &impl ConnectionTrait,
    account: &[Value],
    table: &str,
    columns: &str,
    rows: Vec<Vec<Value>>,
) -> Result<()> {
    let width = columns.split(',').count() + account.len();
    let place = format!("({})", vec!["?"; width].join(","));
    for batch in rows.chunks(BATCH) {
        let mut values = Vec::with_capacity(batch.len() * width);
        for row in batch {
            values.extend(account.iter().cloned());
            values.extend(row.iter().cloned());
        }
        db.execute_raw(sql(
            &format!(
                "INSERT INTO {table}(workspace_id,ad_account_customer_id,{columns}) VALUES{}",
                vec![place.as_str(); batch.len()].join(",")
            ),
            values,
        ))
        .await?;
    }
    Ok(())
}

impl WorkspaceStore<'_> {
    pub async fn breakdown_state(&self, id: &str, kind: Breakdown) -> Result<BreakdownState> {
        let mut values = self.monitoring_account(id);
        values.push(kind.key().into());
        self.db
            .query_one_raw(sql(
                "SELECT first_day,last_day,error FROM breakdown_history WHERE workspace_id=? AND ad_account_customer_id=? AND kind=?",
                values,
            ))
            .await?
            .map_or_else(
                || Ok(BreakdownState::default()),
                |row| {
                    Ok(BreakdownState {
                        first_day: row.try_get("", "first_day")?,
                        last_day: row.try_get("", "last_day")?,
                        error: row.try_get("", "error")?,
                    })
                },
            )
    }

    pub async fn breakdown_states(&self, id: &str) -> Result<BTreeMap<String, BreakdownState>> {
        self.db
            .query_all_raw(sql(
                "SELECT kind,first_day,last_day,error FROM breakdown_history WHERE workspace_id=? AND ad_account_customer_id=?",
                self.monitoring_account(id),
            ))
            .await?
            .into_iter()
            .map(|row| {
                Ok((
                    row.try_get("", "kind")?,
                    BreakdownState {
                        first_day: row.try_get("", "first_day")?,
                        last_day: row.try_get("", "last_day")?,
                        error: row.try_get("", "error")?,
                    },
                ))
            })
            .collect()
    }

    pub async fn breakdown_failed(&self, id: &str, kind: Breakdown, error: &str) -> Result<()> {
        let mut values = self.monitoring_account(id);
        values.extend([kind.key().into(), error.into()]);
        self.db.execute_raw(sql(
            "INSERT INTO breakdown_history(workspace_id,ad_account_customer_id,kind,error) SELECT workspace_id,customer_id,?3,?4 FROM ad_accounts WHERE workspace_id=?1 AND customer_id=?2 ON CONFLICT(workspace_id,ad_account_customer_id,kind) DO UPDATE SET error=excluded.error",
            values,
        )).await?;
        Ok(())
    }

    pub(crate) async fn restart_breakdown(&self, id: &str, kind: Breakdown) -> Result<()> {
        let mut values = self.monitoring_account(id);
        values.push(kind.key().into());
        self.db.execute_raw(sql(
            "UPDATE breakdown_history SET first_day=NULL,last_day=NULL WHERE workspace_id=? AND ad_account_customer_id=? AND kind=?",
            values,
        )).await?;
        Ok(())
    }

    pub async fn save_breakdown(
        &self,
        id: &str,
        from: i64,
        to: i64,
        days: BreakdownDays,
    ) -> Result<()> {
        let account = self.monitoring_account(id);
        let kind = days.kind();
        let txn = begin_write(self.db).await?;
        if txn
            .query_one_raw(sql(
                "SELECT 1 FROM ad_accounts WHERE workspace_id=? AND customer_id=?",
                account.clone(),
            ))
            .await?
            .is_none()
        {
            return Ok(());
        }
        let (table, columns, rows) = match days {
            BreakdownDays::SearchShare(days) => (
                "campaign_search_share_days",
                "day,campaign_id,impressions,shown,shown_under,lost_budget,lost_budget_over,lost_rank,lost_rank_over",
                days.into_iter()
                    .map(|row| {
                        let mut values: Vec<Value> =
                            vec![row.day.into(), row.campaign_id.into(), row.impressions.into()];
                        values.extend(row.shown.columns());
                        values.extend(row.lost_budget.columns());
                        values.extend(row.lost_rank.columns());
                        values
                    })
                    .collect::<Vec<_>>(),
            ),
            BreakdownDays::ConversionActions(days) => {
                let mut merged: BTreeMap<(i64, i64, String), (f64, f64)> = BTreeMap::new();
                for row in days {
                    let total = merged
                        .entry((row.day, row.campaign_id, row.action))
                        .or_default();
                    total.0 += row.conversions;
                    total.1 += row.value;
                }
                (
                    "conversion_action_days",
                    "day,campaign_id,action,conversions,conversions_value",
                    merged
                        .into_iter()
                        .map(|((day, campaign, action), (conversions, value))| {
                            vec![
                                day.into(),
                                campaign.into(),
                                action.into(),
                                conversions.into(),
                                value.into(),
                            ]
                        })
                        .collect(),
                )
            }
            BreakdownDays::Networks(days) => (
                "campaign_network_days",
                "day,campaign_id,network,impressions,clicks,cost_micros,conversions,conversions_value",
                days.into_iter()
                    .map(|row| {
                        let mut values: Vec<Value> =
                            vec![row.day.into(), row.campaign_id.into(), row.network.into()];
                        values.extend(counts_values(&row.counts));
                        values
                    })
                    .collect(),
            ),
        };
        let mut range = account.clone();
        range.extend([from.into(), to.into()]);
        txn.execute_raw(sql(
            &format!("DELETE FROM {table} WHERE workspace_id=? AND ad_account_customer_id=? AND day BETWEEN ? AND ?"),
            range.clone(),
        ))
        .await?;
        insert(&txn, &account, table, columns, rows).await?;
        range.insert(2, kind.key().into());
        txn.execute_raw(sql(
            "INSERT INTO breakdown_history(workspace_id,ad_account_customer_id,kind,first_day,last_day,error) VALUES(?,?,?,?,?,NULL) ON CONFLICT(workspace_id,ad_account_customer_id,kind) DO UPDATE SET first_day=MIN(COALESCE(first_day,excluded.first_day),excluded.first_day),last_day=MAX(COALESCE(last_day,excluded.last_day),excluded.last_day),error=NULL",
            range,
        ))
        .await?;
        txn.commit().await?;
        Ok(())
    }

    pub async fn search_share_totals(
        &self,
        id: &str,
        start: i64,
        end: i64,
    ) -> Result<Vec<SearchShareTotal>> {
        let mut values = self.monitoring_account(id);
        values.extend([start.into(), end.into()]);
        self.db
            .query_all_raw(sql(
                "SELECT campaign_id,COUNT(*) AS days,SUM(shown_under=1 OR lost_budget_over=1 OR lost_rank_over=1) AS limit_days,SUM(CASE WHEN exact THEN 1 ELSE 0 END) AS exact_days,SUM(CASE WHEN exact THEN impressions END) AS impressions,SUM(CASE WHEN exact THEN impressions/shown END) AS eligible,SUM(CASE WHEN exact THEN lost_budget*impressions/shown END) AS lost_budget,SUM(CASE WHEN exact THEN lost_rank*impressions/shown END) AS lost_rank FROM (SELECT *,shown>0 AND lost_budget IS NOT NULL AND lost_rank IS NOT NULL AS exact FROM campaign_search_share_days WHERE workspace_id=? AND ad_account_customer_id=? AND day BETWEEN ? AND ?) GROUP BY campaign_id ORDER BY campaign_id",
                values,
            ))
            .await?
            .into_iter()
            .map(|row| {
                let eligible = row.try_get::<Option<f64>>("", "eligible")?.unwrap_or_default();
                let shares = if eligible > 0.0 {
                    Some(SearchShares {
                        shown: row.try_get::<Option<i64>>("", "impressions")?.unwrap_or_default()
                            as f64
                            / eligible,
                        lost_budget: row
                            .try_get::<Option<f64>>("", "lost_budget")?
                            .unwrap_or_default()
                            / eligible,
                        lost_rank: row.try_get::<Option<f64>>("", "lost_rank")?.unwrap_or_default()
                            / eligible,
                    })
                } else {
                    None
                };
                Ok(SearchShareTotal {
                    campaign_id: row.try_get("", "campaign_id")?,
                    shares,
                    limit_days: row.try_get("", "limit_days")?,
                    exact_days: row.try_get("", "exact_days")?,
                    days: row.try_get("", "days")?,
                })
            })
            .collect()
    }

    pub async fn conversion_action_weeks(
        &self,
        id: &str,
        start: i64,
        end: i64,
    ) -> Result<Vec<(i64, String, f64)>> {
        let mut values = self.monitoring_account(id);
        values.extend([start.into(), end.into()]);
        self.db
            .query_all_raw(sql(
                "SELECT (day-?3)/7 AS week,action,SUM(conversions) AS conversions FROM conversion_action_days WHERE workspace_id=?1 AND ad_account_customer_id=?2 AND day BETWEEN ?3 AND ?4 GROUP BY week,action ORDER BY week,action",
                values,
            ))
            .await?
            .into_iter()
            .map(|row| {
                Ok((
                    row.try_get("", "week")?,
                    row.try_get("", "action")?,
                    row.try_get("", "conversions")?,
                ))
            })
            .collect()
    }

    pub async fn network_totals(
        &self,
        id: &str,
        start: i64,
        end: i64,
        campaign: i64,
    ) -> Result<Vec<NetworkTotal>> {
        let mut values = self.monitoring_account(id);
        values.extend([start.into(), end.into(), campaign.into()]);
        self.db
            .query_all_raw(sql(
                "SELECT network,SUM(cost_micros) AS cost,SUM(clicks) AS clicks,SUM(conversions) AS conversions FROM campaign_network_days WHERE workspace_id=? AND ad_account_customer_id=? AND day BETWEEN ? AND ? AND campaign_id=? GROUP BY network ORDER BY network",
                values,
            ))
            .await?
            .into_iter()
            .map(|row| {
                Ok(NetworkTotal {
                    network: row.try_get("", "network")?,
                    spend: row.try_get::<i64>("", "cost")? as f64 / 1_000_000.0,
                    clicks: row.try_get::<i64>("", "clicks")? as f64,
                    conversions: row.try_get("", "conversions")?,
                })
            })
            .collect()
    }
}
