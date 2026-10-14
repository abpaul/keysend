use loco_rs::Result;
use sea_orm::{ConnectionTrait, Value};

use super::{
    breakdowns::{counts_values, insert},
    sql, Breakdown, WorkspaceStore,
};
use crate::{database::begin_write, monitoring::audit::Counts};

pub type Collected<T> = std::result::Result<T, String>;

#[derive(Clone, Debug)]
pub struct HourTotal {
    pub weekday: i64,
    pub hour: i64,
    pub counts: Counts,
}

#[derive(Clone, Debug)]
pub struct LocationTotal {
    pub id: i64,
    pub name: String,
    pub counts: Counts,
}

#[derive(Clone, Debug, PartialEq)]
pub struct KeywordQuality {
    pub campaign_id: i64,
    pub ad_group_id: i64,
    pub criterion_id: i64,
    pub keyword: String,
    pub match_type: String,
    pub cost: f64,
    pub score: Option<i64>,
    pub ad_relevance: Option<String>,
    pub landing_page: Option<String>,
    pub expected_ctr: Option<String>,
}

#[derive(Clone, Debug)]
pub struct DemographicTotal {
    pub value: String,
    pub counts: Counts,
}

pub struct DeepBreakdowns {
    pub hours: Collected<Vec<HourTotal>>,
    pub locations: Collected<Vec<LocationTotal>>,
    pub keywords: Collected<Vec<KeywordQuality>>,
    pub ages: Collected<Vec<DemographicTotal>>,
    pub genders: Collected<Vec<DemographicTotal>>,
    pub delay: Option<Collected<Vec<(String, f64)>>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct KeywordScore {
    pub quality: KeywordQuality,
    pub previous: Option<i64>,
    pub known_before: bool,
}

type StoredRows = Collected<Vec<Vec<Value>>>;

fn with_counts(mut values: Vec<Value>, counts: &Counts) -> Vec<Value> {
    values.extend(counts_values(counts));
    values
}

fn counts_of(row: &sea_orm::QueryResult) -> Result<Counts> {
    Ok(Counts {
        impressions: row.try_get::<i64>("", "impressions")? as f64,
        clicks: row.try_get::<i64>("", "clicks")? as f64,
        cost: row.try_get::<i64>("", "cost_micros")? as f64 / 1_000_000.0,
        conversions: row.try_get("", "conversions")?,
        value: row.try_get("", "conversions_value")?,
    })
}

const COUNT_COLUMNS: &str = "impressions,clicks,cost_micros,conversions,conversions_value";

impl WorkspaceStore<'_> {
    pub async fn save_deep_breakdowns(
        &self,
        id: &str,
        day: i64,
        rows: &DeepBreakdowns,
    ) -> Result<()> {
        let account = self.monitoring_account(id);
        let txn = begin_write(self.db).await?;
        let Some(exists) = txn
            .query_one_raw(sql(
                "SELECT (SELECT day FROM deep_checks WHERE workspace_id=?1 AND ad_account_customer_id=?2) AS checked FROM ad_accounts WHERE workspace_id=?1 AND customer_id=?2",
                account.clone(),
            ))
            .await?
        else {
            return Ok(());
        };
        let before = exists
            .try_get::<Option<i64>>("", "checked")?
            .filter(|checked| *checked < day);
        let mut parts: Vec<(Breakdown, &str, String, Option<StoredRows>)> = Vec::new();
        parts.push((
            Breakdown::Hours,
            "hour_totals",
            format!("day,weekday,hour,{COUNT_COLUMNS}"),
            Some(rows.hours.clone().map(|rows| {
                rows.iter()
                    .map(|row| {
                        with_counts(
                            vec![day.into(), row.weekday.into(), row.hour.into()],
                            &row.counts,
                        )
                    })
                    .collect()
            })),
        ));
        parts.push((
            Breakdown::Locations,
            "location_totals",
            format!("day,location_id,name,{COUNT_COLUMNS}"),
            Some(rows.locations.clone().map(|rows| {
                rows.iter()
                    .map(|row| {
                        with_counts(
                            vec![day.into(), row.id.into(), row.name.clone().into()],
                            &row.counts,
                        )
                    })
                    .collect()
            })),
        ));
        parts.push((
            Breakdown::KeywordQuality,
            "keyword_quality",
            "day,campaign_id,ad_group_id,criterion_id,keyword,match_type,cost_micros,quality_score,ad_relevance,landing_page,expected_ctr".into(),
            Some(rows.keywords.clone().map(|rows| {
                rows.iter()
                    .map(|row| {
                        vec![
                            day.into(),
                            row.campaign_id.into(),
                            row.ad_group_id.into(),
                            row.criterion_id.into(),
                            row.keyword.clone().into(),
                            row.match_type.clone().into(),
                            ((row.cost * 1_000_000.0).round() as i64).into(),
                            row.score.into(),
                            row.ad_relevance.clone().into(),
                            row.landing_page.clone().into(),
                            row.expected_ctr.clone().into(),
                        ]
                    })
                    .collect()
            })),
        ));
        for (kind, name, totals) in [
            (Breakdown::Age, "age", &rows.ages),
            (Breakdown::Gender, "gender", &rows.genders),
        ] {
            parts.push((
                kind,
                "demographic_totals",
                format!("day,kind,value,{COUNT_COLUMNS}"),
                Some(totals.clone().map(|rows| {
                    rows.iter()
                        .map(|row| {
                            with_counts(
                                vec![day.into(), name.into(), row.value.clone().into()],
                                &row.counts,
                            )
                        })
                        .collect()
                })),
            ));
        }
        parts.push((
            Breakdown::ConversionDelay,
            "conversion_lag_totals",
            "day,bucket,conversions".into(),
            rows.delay.clone().map(|delay| {
                delay.map(|rows| {
                    rows.into_iter()
                        .map(|(bucket, conversions)| {
                            vec![day.into(), bucket.into(), conversions.into()]
                        })
                        .collect()
                })
            }),
        ));
        for (kind, table, columns, collected) in parts {
            let mut old = account.clone();
            let filter = match kind {
                Breakdown::KeywordQuality => {
                    old.push(before.into());
                    " AND (?3 IS NULL OR day<>?3)"
                }
                Breakdown::Age => " AND kind='age'",
                Breakdown::Gender => " AND kind='gender'",
                _ => "",
            };
            txn.execute_raw(sql(
                &format!("DELETE FROM {table} WHERE workspace_id=?1 AND ad_account_customer_id=?2{filter}"),
                old,
            ))
            .await?;
            let mut state = account.clone();
            state.push(kind.key().into());
            match collected {
                None => {
                    txn.execute_raw(sql(
                        "DELETE FROM breakdown_history WHERE workspace_id=? AND ad_account_customer_id=? AND kind=?",
                        state,
                    ))
                    .await?;
                }
                Some(collected) => {
                    state.extend([day.into(), collected.as_ref().err().cloned().into()]);
                    txn.execute_raw(sql(
                        "INSERT INTO breakdown_history(workspace_id,ad_account_customer_id,kind,first_day,last_day,error) VALUES(?1,?2,?3,?4,?4,?5) ON CONFLICT(workspace_id,ad_account_customer_id,kind) DO UPDATE SET first_day=excluded.first_day,last_day=excluded.last_day,error=excluded.error",
                        state,
                    ))
                    .await?;
                    if let Ok(rows) = collected {
                        insert(&txn, &account, table, &columns, rows).await?;
                    }
                }
            }
        }
        txn.commit().await?;
        Ok(())
    }

    pub async fn hour_totals(&self, id: &str) -> Result<Vec<HourTotal>> {
        self.db
            .query_all_raw(sql(
                &format!("SELECT weekday,hour,{COUNT_COLUMNS} FROM hour_totals WHERE workspace_id=? AND ad_account_customer_id=? ORDER BY weekday,hour"),
                self.monitoring_account(id),
            ))
            .await?
            .into_iter()
            .map(|row| {
                Ok(HourTotal {
                    weekday: row.try_get("", "weekday")?,
                    hour: row.try_get("", "hour")?,
                    counts: counts_of(&row)?,
                })
            })
            .collect()
    }

    pub async fn location_totals(&self, id: &str) -> Result<Vec<LocationTotal>> {
        self.db
            .query_all_raw(sql(
                &format!("SELECT location_id,name,{COUNT_COLUMNS} FROM location_totals WHERE workspace_id=? AND ad_account_customer_id=? ORDER BY cost_micros DESC,name"),
                self.monitoring_account(id),
            ))
            .await?
            .into_iter()
            .map(|row| {
                Ok(LocationTotal {
                    id: row.try_get("", "location_id")?,
                    name: row.try_get("", "name")?,
                    counts: counts_of(&row)?,
                })
            })
            .collect()
    }

    pub async fn demographic_totals(&self, id: &str) -> Result<Vec<(String, DemographicTotal)>> {
        self.db
            .query_all_raw(sql(
                &format!("SELECT kind,value,{COUNT_COLUMNS} FROM demographic_totals WHERE workspace_id=? AND ad_account_customer_id=? ORDER BY kind,value"),
                self.monitoring_account(id),
            ))
            .await?
            .into_iter()
            .map(|row| {
                Ok((
                    row.try_get("", "kind")?,
                    DemographicTotal {
                        value: row.try_get("", "value")?,
                        counts: counts_of(&row)?,
                    },
                ))
            })
            .collect()
    }

    pub async fn conversion_delay(&self, id: &str) -> Result<Option<(f64, f64)>> {
        let row = self
            .db
            .query_one_raw(sql(
                "SELECT SUM(conversions) AS conversions,SUM(CASE WHEN bucket NOT IN ('LESS_THAN_ONE_DAY','ONE_TO_TWO_DAYS') THEN conversions ELSE 0 END) AS late FROM conversion_lag_totals WHERE workspace_id=? AND ad_account_customer_id=? AND bucket NOT IN ('UNKNOWN','UNSPECIFIED')",
                self.monitoring_account(id),
            ))
            .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        Ok(row
            .try_get::<Option<f64>>("", "conversions")?
            .zip(row.try_get::<Option<f64>>("", "late")?))
    }

    pub async fn keyword_scores(
        &self,
        id: &str,
        campaign: i64,
        limit: u32,
    ) -> Result<(Vec<KeywordScore>, bool)> {
        let mut values = self.monitoring_account(id);
        values.extend([campaign.into(), limit.into()]);
        let rows = self
            .db
            .query_all_raw(sql(
                "WITH days AS (SELECT MAX(day) AS last,MIN(day) AS first FROM keyword_quality WHERE workspace_id=?1 AND ad_account_customer_id=?2) SELECT cur.ad_group_id,cur.criterion_id,cur.keyword,cur.match_type,cur.cost_micros,cur.quality_score,cur.ad_relevance,cur.landing_page,cur.expected_ctr,prev.quality_score AS previous,prev.day IS NOT NULL AS known_before,days.first<days.last AS compared FROM days JOIN keyword_quality cur ON cur.workspace_id=?1 AND cur.ad_account_customer_id=?2 AND cur.day=days.last AND cur.campaign_id=?3 LEFT JOIN keyword_quality prev ON prev.workspace_id=cur.workspace_id AND prev.ad_account_customer_id=cur.ad_account_customer_id AND prev.day<cur.day AND prev.campaign_id=cur.campaign_id AND prev.ad_group_id=cur.ad_group_id AND prev.criterion_id=cur.criterion_id ORDER BY cur.cost_micros DESC,cur.keyword,cur.ad_group_id,cur.criterion_id LIMIT ?4",
                values,
            ))
            .await?;
        let compared = rows
            .first()
            .map(|row| row.try_get::<bool>("", "compared"))
            .transpose()?
            .unwrap_or_default();
        let scores = rows
            .into_iter()
            .map(|row| {
                Ok(KeywordScore {
                    quality: KeywordQuality {
                        campaign_id: campaign,
                        ad_group_id: row.try_get("", "ad_group_id")?,
                        criterion_id: row.try_get("", "criterion_id")?,
                        keyword: row.try_get("", "keyword")?,
                        match_type: row.try_get("", "match_type")?,
                        cost: row.try_get::<i64>("", "cost_micros")? as f64 / 1_000_000.0,
                        score: row.try_get("", "quality_score")?,
                        ad_relevance: row.try_get("", "ad_relevance")?,
                        landing_page: row.try_get("", "landing_page")?,
                        expected_ctr: row.try_get("", "expected_ctr")?,
                    },
                    previous: row.try_get("", "previous")?,
                    known_before: row.try_get("", "known_before")?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok((scores, compared))
    }
}
