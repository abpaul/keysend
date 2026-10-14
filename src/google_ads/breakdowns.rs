use chrono::{Days, NaiveDate};
use serde_json::Value;

use super::{counts, numeric, ranges, resource_id, text, Client, Failure, COUNTS};
use crate::{
    monitoring::charts::WEEKS,
    workspace::{
        history::day_number,
        monitoring::{ActionDay, Breakdown, BreakdownDays, NetworkDay, SearchShareDay, Share},
        WorkspaceStore,
    },
};

const DAYS: u64 = WEEKS as u64 * 7;

/// Google Ads reports a search impression share below this as 0.0999 and gives no exact value.
const SHOWN_FLOOR: f64 = 0.1;
/// Google Ads reports a lost search impression share above this as 0.9001 and gives no exact
/// value.
const LOST_CEILING: f64 = 0.9;

const KINDS: [Breakdown; 3] = [
    Breakdown::SearchShare,
    Breakdown::ConversionActions,
    Breakdown::Networks,
];

pub(crate) fn query(kind: Breakdown, from: NaiveDate, to: NaiveDate) -> String {
    let days = format!("segments.date BETWEEN '{from}' AND '{to}'");
    match kind {
        Breakdown::SearchShare => format!("SELECT campaign.id, segments.date, metrics.impressions, metrics.search_impression_share, metrics.search_budget_lost_impression_share, metrics.search_rank_lost_impression_share FROM campaign WHERE campaign.advertising_channel_type = 'SEARCH' AND {days}"),
        Breakdown::ConversionActions => format!("SELECT campaign.id, segments.date, segments.conversion_action_name, metrics.conversions, metrics.conversions_value FROM campaign WHERE {days}"),
        _ => format!("SELECT campaign.id, segments.date, segments.ad_network_type, {COUNTS} FROM campaign WHERE {days}"),
    }
}

fn shown(value: Option<&Value>) -> Result<Share, Failure> {
    Ok(match value.filter(|value| !value.is_null()) {
        None => Share::Unknown,
        Some(value) => match numeric(value)? {
            share if share < SHOWN_FLOOR => Share::OutsideExactRange,
            share => Share::Exact(share),
        },
    })
}

fn lost(value: Option<&Value>) -> Result<Share, Failure> {
    Ok(match value.filter(|value| !value.is_null()) {
        None => Share::Unknown,
        Some(value) => match numeric(value)? {
            share if share > LOST_CEILING => Share::OutsideExactRange,
            share => Share::Exact(share),
        },
    })
}

fn rows(kind: Breakdown, found: &[Value]) -> Result<BreakdownDays, Failure> {
    let key = |row: &Value| {
        NaiveDate::parse_from_str(&text(row, "/segments/date"), "%Y-%m-%d")
            .ok()
            .map(day_number)
            .zip(resource_id(row, "/campaign/id"))
            .ok_or(Failure::Unavailable)
    };
    Ok(match kind {
        Breakdown::SearchShare => BreakdownDays::SearchShare(
            found
                .iter()
                .map(|row| {
                    let (day, campaign_id) = key(row)?;
                    Ok(SearchShareDay {
                        day,
                        campaign_id,
                        impressions: numeric(&row["metrics"]["impressions"])?.round() as i64,
                        shown: shown(row.pointer("/metrics/searchImpressionShare"))?,
                        lost_budget: lost(row.pointer("/metrics/searchBudgetLostImpressionShare"))?,
                        lost_rank: lost(row.pointer("/metrics/searchRankLostImpressionShare"))?,
                    })
                })
                .collect::<Result<_, Failure>>()?,
        ),
        Breakdown::ConversionActions => BreakdownDays::ConversionActions(
            found
                .iter()
                .map(|row| {
                    let (day, campaign_id) = key(row)?;
                    Ok(ActionDay {
                        day,
                        campaign_id,
                        action: row
                            .pointer("/segments/conversionActionName")
                            .and_then(Value::as_str)
                            .ok_or(Failure::Unavailable)?
                            .to_owned(),
                        conversions: numeric(&row["metrics"]["conversions"])?,
                        value: numeric(&row["metrics"]["conversionsValue"])?,
                    })
                })
                .collect::<Result<_, Failure>>()?,
        ),
        _ => BreakdownDays::Networks(
            found
                .iter()
                .map(|row| {
                    let (day, campaign_id) = key(row)?;
                    Ok(NetworkDay {
                        day,
                        campaign_id,
                        network: row
                            .pointer("/segments/adNetworkType")
                            .and_then(Value::as_str)
                            .ok_or(Failure::Unavailable)?
                            .to_owned(),
                        counts: counts(row)?,
                    })
                })
                .collect::<Result<_, Failure>>()?,
        ),
    })
}

async fn collect_kind(
    client: &Client<'_, '_>,
    store: &WorkspaceStore<'_>,
    id: &str,
    kind: Breakdown,
    end: NaiveDate,
    recent_only: bool,
) -> Result<(), Failure> {
    let oldest = end.checked_sub_days(Days::new(DAYS - 1)).unwrap_or(end);
    let mut state = store
        .breakdown_state(id, kind)
        .await
        .map_err(|_| Failure::Unavailable)?;
    let stale = day_number(end.checked_sub_days(Days::new(29)).unwrap_or(end));
    if state.last_day.is_some_and(|last| last < stale) {
        store
            .restart_breakdown(id, kind)
            .await
            .map_err(|_| Failure::Unavailable)?;
        state.first_day = None;
    }
    let first = state.first_day.and_then(crate::workspace::history::date_of);
    for (from, to) in ranges(end, oldest, first, recent_only)? {
        let found = client.query_later(&query(kind, from, to)).await?;
        let Ok(days) = rows(kind, &found) else {
            client
                .record(&format!("{}: a row cannot be read", kind.key()))
                .await;
            return Err(Failure::Unavailable);
        };
        store
            .save_breakdown(id, day_number(from), day_number(to), days)
            .await
            .map_err(|_| Failure::Unavailable)?;
    }
    Ok(())
}

pub(super) async fn collect(
    client: &Client<'_, '_>,
    store: &WorkspaceStore<'_>,
    id: &str,
    end: NaiveDate,
    recent_only: bool,
) {
    for kind in KINDS {
        if kind == Breakdown::ConversionActions {
            match store.conversion_tracking(id, day_number(end)).await {
                Ok((_, true)) => {}
                Ok((_, false)) => continue,
                Err(_) => {
                    tracing::warn!("Conversion tracking could not be read for the breakdowns");
                    continue;
                }
            }
        }
        let Err(failure) = collect_kind(client, store, id, kind, end, recent_only).await else {
            continue;
        };
        if matches!(failure, Failure::DailyLimit) {
            tracing::warn!(
                kind = kind.key(),
                "Breakdown waits for the next run: the daily Google Ads operations are used"
            );
            return;
        }
        tracing::warn!(kind = kind.key(), failure = ?failure, "Breakdown was not collected");
        if store
            .breakdown_failed(id, kind, failure.message())
            .await
            .is_err()
        {
            tracing::warn!(kind = kind.key(), "Breakdown failure could not be stored");
        }
        if matches!(
            failure,
            Failure::Quota | Failure::Reconnect | Failure::Disabled
        ) {
            return;
        }
    }
}
