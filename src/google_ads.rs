use std::collections::{BTreeMap, BTreeSet};

use chrono::{Days, Months, NaiveDate};
use loco_rs::{Error, Result};
use serde_json::{json, Value};

use crate::{
    google,
    monitoring::{
        audit::{AccountChange, Counts},
        metrics::{english_currency_prefix, english_number},
    },
    settings::Settings,
    workspace::{history::local_time, AdAccount, Operation, WorkspaceStore},
};

mod breakdowns;
pub(crate) mod deep_breakdowns;

pub(crate) const COUNTS: &str = "metrics.impressions, metrics.clicks, metrics.conversions, metrics.cost_micros, metrics.conversions_value";

#[derive(Debug)]
pub(crate) enum Failure {
    Reconnect,
    Disabled,
    Quota,
    DailyLimit,
    Unsupported,
    Unavailable,
}

impl Failure {
    fn message(&self) -> &'static str {
        match self {
            Self::Reconnect => "Connect Google Ads again.",
            Self::Disabled => "This Ad Account is not enabled in Google Ads.",
            Self::Quota | Self::DailyLimit => {
                "Google Ads quota or rate limit reached. Try again later."
            }
            Self::Unsupported => "Google Ads does not support this request.",
            Self::Unavailable => "Google Ads data is unavailable. Try again later.",
        }
    }
}

impl From<Failure> for Error {
    fn from(value: Failure) -> Self {
        Self::Message(value.message().into())
    }
}

pub(crate) struct Client<'a, 'db> {
    settings: &'a Settings,
    store: &'a WorkspaceStore<'db>,
    ad: AdAccount,
    access: String,
}

impl<'a, 'db> Client<'a, 'db> {
    pub(crate) async fn connect(
        settings: &'a Settings,
        store: &'a WorkspaceStore<'db>,
        id: &str,
    ) -> std::result::Result<Self, Failure> {
        let ad = store
            .ad_accounts()
            .await
            .map_err(|_| Failure::Unavailable)?
            .into_iter()
            .find(|ad| ad.customer_id == id)
            .ok_or(Failure::Reconnect)?;
        let key = settings.master_key.as_ref().ok_or(Failure::Unavailable)?;
        let refresh = store
            .refresh_token(&ad.google_connection_id, key)
            .await
            .map_err(|_| Failure::Unavailable)?
            .ok_or(Failure::Reconnect)?;
        let access = match google::refresh(settings, &refresh)
            .await
            .map_err(|_| Failure::Unavailable)?
        {
            google::Refreshed::AccessToken(access) => access,
            google::Refreshed::Revoked => return Err(Failure::Reconnect),
        };
        Ok(Self {
            settings,
            store,
            ad,
            access,
        })
    }

    async fn record(&self, detail: &str) {
        if self
            .store
            .record_google_failure(&self.ad.customer_id, detail)
            .await
            .is_err()
        {
            tracing::warn!("Google failure detail could not be stored");
        }
    }

    pub(crate) async fn query_daily_history(
        &self,
        query: &str,
    ) -> std::result::Result<Vec<Value>, Failure> {
        self.run(query, Operation::Daily).await
    }

    pub(crate) async fn query_later(
        &self,
        query: &str,
    ) -> std::result::Result<Vec<Value>, Failure> {
        self.run(query, Operation::Later).await
    }

    async fn run(
        &self,
        query: &str,
        operation: Operation,
    ) -> std::result::Result<Vec<Value>, Failure> {
        let query_kind = if query.contains("customer.currency_code") {
            "metadata"
        } else if query.contains("segments.date, segments.device") {
            "history"
        } else if query.starts_with("SELECT campaign.id, segments.date, ") {
            "breakdowns"
        } else if query.contains("campaign_budget.amount_micros") {
            "campaigns"
        } else if query.contains("change_event.old_resource") {
            "changes"
        } else {
            "deep_check"
        };
        let mut token = String::new();
        let mut seen = BTreeSet::new();
        let mut rows = Vec::new();
        loop {
            let claimed = self
                .store
                .claim_google_operation(self.settings.google.daily_limit, operation)
                .await
                .map_err(|error| {
                    tracing::error!(%error, "Google Ads operation could not be counted");
                    Failure::Unavailable
                })?;
            if !claimed {
                let limit = self.settings.google.daily_limit;
                match operation {
                    Operation::Daily => tracing::error!(
                        limit,
                        query_kind,
                        "Google Ads daily operation limit reached; no request was sent"
                    ),
                    Operation::Later => tracing::warn!(
                        limit,
                        query_kind,
                        "Google Ads daily operations left are kept for the daily Account history; no request was sent"
                    ),
                }
                return Err(Failure::DailyLimit);
            }
            let mut body = json!({"query":query});
            if !token.is_empty() {
                body["pageToken"] = json!(token);
            }
            let mut request = self
                .settings
                .http
                .post(format!(
                    "{}/customers/{}/googleAds:search",
                    self.settings.google.ads_api_base.trim_end_matches('/'),
                    self.ad.customer_id
                ))
                .bearer_auth(&self.access)
                .json(&body);
            if let Some(manager) = &self.ad.login_customer_id {
                request = request.header("login-customer-id", manager);
            }
            let resource = query
                .split_once(" FROM ")
                .and_then(|(_, tail)| tail.split_whitespace().next())
                .filter(|resource| {
                    matches!(
                        *resource,
                        "customer"
                            | "campaign"
                            | "keyword_view"
                            | "search_term_view"
                            | "ad_group_ad"
                            | "landing_page_view"
                            | "change_event"
                            | "geographic_view"
                            | "geo_target_constant"
                            | "age_range_view"
                            | "gender_view"
                    )
                })
                .unwrap_or("other");
            let response = request.send().await.map_err(|error| {
                tracing::warn!(
                    resource,
                    query_kind,
                    timeout = error.is_timeout(),
                    connection = error.is_connect(),
                    "Google Metrics transport failed"
                );
                Failure::Unavailable
            });
            let response = match response {
                Ok(response) => response,
                Err(failure) => {
                    self.record(&format!("{resource} {query_kind}: no response"))
                        .await;
                    return Err(failure);
                }
            };
            let status = response.status();
            if !status.is_success() {
                let body: Value = response.json().await.unwrap_or(Value::Null);
                let codes = body["error"]["details"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .flat_map(|detail| detail["errors"].as_array().into_iter().flatten())
                    .filter_map(|error| error["errorCode"].as_object())
                    .flat_map(|code| code.values())
                    .filter_map(Value::as_str)
                    .collect::<BTreeSet<_>>();
                let diagnostic_codes = codes
                    .iter()
                    .map(|code| match *code {
                        "RESOURCE_EXHAUSTED"
                        | "RESOURCE_TEMPORARILY_EXHAUSTED"
                        | "CUSTOMER_NOT_ENABLED"
                        | "UNRECOGNIZED_FIELD"
                        | "PROHIBITED_FIELD_IN_SELECT_CLAUSE"
                        | "PROHIBITED_SEGMENT_WITH_METRIC_IN_SELECT_OR_WHERE_CLAUSE"
                        | "PROHIBITED_FIELD_COMBINATION_IN_SELECT_CLAUSE"
                        | "PROHIBITED_FIELD_OR_SEGMENT_WITH_METRIC"
                        | "PROHIBITED_METRIC_IN_SELECT_OR_WHERE_CLAUSE"
                        | "PROHIBITED_RESOURCE_TYPE_IN_SELECT_CLAUSE"
                        | "PROHIBITED_SEGMENT_IN_SELECT_OR_WHERE_CLAUSE"
                        | "MISALIGNED_DATE_FOR_FILTER"
                        | "REQUESTED_METRICS_FOR_MANAGER"
                        | "REQUIRED_SEGMENT_FIELD_MISSING" => *code,
                        _ => "OTHER",
                    })
                    .collect::<BTreeSet<_>>();
                tracing::warn!(resource, query_kind, http_status = status.as_u16(), codes = ?diagnostic_codes, "Google Metrics request failed");
                self.record(&format!(
                    "{resource} {query_kind}: HTTP {} {}",
                    status.as_u16(),
                    diagnostic_codes
                        .iter()
                        .copied()
                        .collect::<Vec<_>>()
                        .join(" ")
                ))
                .await;
                return Err(
                    if status == 429
                        || codes.contains("RESOURCE_EXHAUSTED")
                        || codes.contains("RESOURCE_TEMPORARILY_EXHAUSTED")
                    {
                        Failure::Quota
                    } else if codes.contains("CUSTOMER_NOT_ENABLED") {
                        Failure::Disabled
                    } else if status == 401 || status == 403 {
                        Failure::Reconnect
                    } else if codes.contains("UNRECOGNIZED_FIELD")
                        || codes.contains("PROHIBITED_FIELD_IN_SELECT_CLAUSE")
                        || codes
                            .contains("PROHIBITED_SEGMENT_WITH_METRIC_IN_SELECT_OR_WHERE_CLAUSE")
                    {
                        Failure::Unsupported
                    } else {
                        Failure::Unavailable
                    },
                );
            }
            let body: Value = response.json().await.map_err(|_| {
                tracing::warn!(resource, "Google Metrics response decoding failed");
                Failure::Unavailable
            })?;
            if let Some(result) = body["results"].as_array() {
                rows.extend(result.iter().cloned());
            }
            if rows.len() > 100_000 {
                tracing::warn!(
                    resource,
                    query_kind,
                    "Google Metrics response has too many rows"
                );
                self.record(&format!("{resource} {query_kind}: more than 100000 rows"))
                    .await;
                return Err(Failure::Unavailable);
            }
            token = body["nextPageToken"].as_str().unwrap_or_default().into();
            if token.is_empty() {
                return Ok(rows);
            }
            if !seen.insert(token.clone()) {
                tracing::warn!(resource, query_kind, "Google Metrics page token repeated");
                self.record(&format!("{resource} {query_kind}: page token repeated"))
                    .await;
                return Err(Failure::Unavailable);
            }
        }
    }

    pub(crate) async fn metadata(&self) -> std::result::Result<(String, String), Failure> {
        let rows = self
            .query_daily_history(
                "SELECT customer.currency_code, customer.time_zone, customer.status FROM customer",
            )
            .await?;
        let row = rows.first().ok_or(Failure::Disabled)?;
        if row["customer"]["status"] != "ENABLED" {
            return Err(Failure::Disabled);
        }
        let currency = row["customer"]["currencyCode"]
            .as_str()
            .filter(|s| s.len() == 3 && s.bytes().all(|c| c.is_ascii_uppercase()))
            .ok_or(Failure::Unavailable)?
            .to_owned();
        let zone = row["customer"]["timeZone"]
            .as_str()
            .ok_or(Failure::Unavailable)?
            .to_owned();
        local_time(crate::settings::wall_clock(), &zone).map_err(|_| Failure::Unavailable)?;
        self.store
            .save_ad_account_metadata(&self.ad.customer_id, &currency, &zone)
            .await
            .map_err(|_| Failure::Unavailable)?;
        Ok((currency, zone))
    }
}

pub async fn today(settings: &Settings, store: &WorkspaceStore<'_>, id: &str) -> Result<NaiveDate> {
    let client = Client::connect(settings, store, id).await?;
    let (_, zone) = client.metadata().await?;
    Ok(local_time(settings.now(), &zone)
        .map_err(|_| Failure::Unavailable)?
        .date_naive())
}

pub(crate) enum ChangeKind {
    Budget { from: Option<f64>, to: Option<f64> },
    Status { from: String, to: String },
    BidStrategy { from: String, to: String },
    TargetCost { from: Option<f64>, to: Option<f64> },
}

pub(crate) fn change_kind(change: &crate::workspace::history::AccountChange) -> Option<ChangeKind> {
    let old: Value = serde_json::from_str(&change.old_value).unwrap_or(Value::Null);
    let new: Value = serde_json::from_str(&change.new_value).unwrap_or(Value::Null);
    let pair = |pointer: &str| {
        Some((
            old.pointer(pointer).filter(|value| !value.is_null())?,
            new.pointer(pointer).filter(|value| !value.is_null())?,
        ))
    };
    let label = |value: &Value| {
        value
            .as_str()
            .unwrap_or_default()
            .to_lowercase()
            .replace('_', " ")
    };
    let amount = |value: &Value| numeric(value).ok().map(|value| value / 1_000_000.0);
    if let Some((a, b)) = pair("/campaignBudget/amountMicros") {
        Some(ChangeKind::Budget {
            from: amount(a),
            to: amount(b),
        })
    } else if let Some((a, b)) = pair("/campaign/status") {
        Some(ChangeKind::Status {
            from: label(a),
            to: label(b),
        })
    } else if let Some((a, b)) = pair("/campaign/biddingStrategyType") {
        Some(ChangeKind::BidStrategy {
            from: label(a),
            to: label(b),
        })
    } else if let Some((a, b)) = pair("/campaign/targetCpa/targetCpaMicros")
        .or_else(|| pair("/campaign/maximizeConversions/targetCpaMicros"))
    {
        Some(ChangeKind::TargetCost {
            from: amount(a),
            to: amount(b),
        })
    } else {
        None
    }
}

pub(crate) fn change_text(
    change: &crate::workspace::history::AccountChange,
    names: &BTreeMap<i64, String>,
    currency: &str,
) -> Option<AccountChange> {
    let text = match change_kind(change)? {
        ChangeKind::Budget { from, to } => format!(
            "Daily budget changed from {} to {}",
            money(from, currency),
            money(to, currency)
        ),
        ChangeKind::Status { from, to } => {
            format!("Campaign status changed from {from} to {to}")
        }
        ChangeKind::BidStrategy { from, to } => {
            format!("Bid strategy changed from {from} to {to}")
        }
        ChangeKind::TargetCost { from, to } => format!(
            "Target cost per conversion changed from {} to {}",
            money(from, currency),
            money(to, currency)
        ),
    };
    Some(AccountChange {
        date: change.changed_at.chars().take(10).collect(),
        campaign: change
            .campaign_id
            .and_then(|id| names.get(&id).cloned())
            .unwrap_or_default(),
        campaign_id: change.campaign_id,
        text,
    })
}

fn device_number(name: &str) -> i64 {
    match name {
        "MOBILE" => 1,
        "DESKTOP" => 2,
        "TABLET" => 3,
        "CONNECTED_TV" => 4,
        _ => 0,
    }
}

pub(crate) fn resource_id(row: &Value, pointer: &str) -> Option<i64> {
    let value = row.pointer(pointer)?;
    value.as_i64().or_else(|| {
        value
            .as_str()
            .and_then(|text| text.rsplit('/').next())
            .and_then(|id| id.parse().ok())
    })
}

fn ranges(
    end: NaiveDate,
    oldest: NaiveDate,
    collected_first: Option<NaiveDate>,
    recent_only: bool,
) -> std::result::Result<Vec<(NaiveDate, NaiveDate)>, Failure> {
    let mut ranges = Vec::new();
    let recent = end
        .checked_sub_days(Days::new(59))
        .ok_or(Failure::Unavailable)?
        .max(oldest);
    ranges.push((
        if collected_first.is_some() {
            end.checked_sub_days(Days::new(29))
                .ok_or(Failure::Unavailable)?
                .max(oldest)
        } else {
            recent
        },
        end,
    ));
    let mut before = collected_first
        .unwrap_or(recent)
        .pred_opt()
        .ok_or(Failure::Unavailable)?;
    while before >= oldest {
        let start = before
            .checked_sub_days(Days::new(91))
            .ok_or(Failure::Unavailable)?
            .max(oldest);
        ranges.push((start, before));
        before = start.pred_opt().ok_or(Failure::Unavailable)?;
    }
    if recent_only {
        ranges.truncate(1);
    }
    Ok(ranges)
}

async fn history(
    client: &Client<'_, '_>,
    settings: &Settings,
    store: &WorkspaceStore<'_>,
    id: &str,
    recent_only: bool,
) -> std::result::Result<(), Failure> {
    let (_, zone) = match store
        .ad_account_metadata(id)
        .await
        .map_err(|_| Failure::Unavailable)?
    {
        Some(metadata) => metadata,
        None => client.metadata().await?,
    };
    let today = local_time(settings.now(), &zone)
        .map_err(|_| Failure::Unavailable)?
        .date_naive();
    let end = today.pred_opt().ok_or(Failure::Unavailable)?;
    let oldest = today
        .checked_sub_months(Months::new(24))
        .ok_or(Failure::Unavailable)?;
    let mut state = store
        .history_state(id)
        .await
        .map_err(|_| Failure::Unavailable)?;
    if state
        .as_ref()
        .and_then(|state| state.last_day)
        .is_some_and(|last| last < end.checked_sub_days(Days::new(29)).unwrap_or(end))
    {
        store
            .restart_monitoring_history(id)
            .await
            .map_err(|_| Failure::Unavailable)?;
        state = None;
    }
    let campaigns = client
        .query_daily_history("SELECT campaign.id, campaign.name, campaign.status, campaign.advertising_channel_type, campaign.bidding_strategy_type, campaign_budget.amount_micros FROM campaign")
        .await?
        .iter()
        .filter_map(|row| {
            Some(crate::workspace::history::Campaign {
                id: resource_id(row, "/campaign/id")?,
                name: text(row, "/campaign/name"),
                status: text(row, "/campaign/status"),
                channel_type: text(row, "/campaign/advertisingChannelType"),
                bidding_strategy: text(row, "/campaign/biddingStrategyType"),
                budget_micros: resource_id(row, "/campaignBudget/amountMicros"),
            })
        })
        .collect::<Vec<_>>();
    let known_campaigns = store
        .campaigns(id)
        .await
        .map_err(|_| Failure::Unavailable)?
        .into_iter()
        .map(|campaign| campaign.id)
        .collect::<BTreeSet<_>>();
    let new_campaigns = campaigns
        .iter()
        .filter(|campaign| !known_campaigns.contains(&campaign.id))
        .count();
    let changes_from = today
        .checked_sub_days(Days::new(28))
        .ok_or(Failure::Unavailable)?;
    let mut changes = Vec::new();
    let mut until = format!("{today} 23:59:59");
    for _ in 0..5 {
        let page = client
            .query_daily_history(&format!("SELECT change_event.change_date_time, change_event.change_resource_type, change_event.resource_change_operation, change_event.changed_fields, change_event.old_resource, change_event.new_resource, change_event.resource_name, change_event.campaign FROM change_event WHERE change_event.change_date_time >= '{changes_from} 00:00:00' AND change_event.change_date_time <= '{until}' ORDER BY change_event.change_date_time DESC LIMIT 10000"))
            .await?;
        let full = page.len() >= 10_000;
        let oldest = page
            .last()
            .map(|row| text(row, "/changeEvent/changeDateTime"));
        changes.extend(page.iter().map(|row| {
            let whole = |pointer: &str| {
                row.pointer(pointer)
                    .filter(|value| !value.is_null())
                    .map(Value::to_string)
                    .filter(|text| text.len() <= 4000)
                    .unwrap_or_default()
            };
            crate::workspace::history::AccountChange {
                changed_at: text(row, "/changeEvent/changeDateTime"),
                resource_name: text(row, "/changeEvent/resourceName"),
                resource_type: text(row, "/changeEvent/changeResourceType"),
                operation: text(row, "/changeEvent/resourceChangeOperation"),
                campaign_id: resource_id(row, "/changeEvent/campaign"),
                changed_fields: row
                    .pointer("/changeEvent/changedFields")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                old_value: whole("/changeEvent/oldResource"),
                new_value: whole("/changeEvent/newResource"),
            }
        }));
        match oldest {
            Some(oldest) if full && oldest != until => until = oldest,
            _ => {
                if full {
                    tracing::warn!("Account changes were not collected completely");
                }
                break;
            }
        }
    }
    let ranges = ranges(
        end,
        oldest,
        state.as_ref().and_then(|state| state.first_day),
        recent_only,
    )?;
    for (index, (from, to)) in ranges.into_iter().enumerate() {
        let mut days = Vec::new();
        let query = format!("SELECT campaign.id, segments.date, segments.device, {COUNTS} FROM campaign WHERE segments.date BETWEEN '{from}' AND '{to}'");
        let found = if index == 0 {
            client.query_daily_history(&query).await?
        } else {
            match client.query_later(&query).await {
                Err(Failure::DailyLimit) => {
                    tracing::warn!(
                        "The older days of the Account history wait for the next run: the daily Google Ads operations are used"
                    );
                    return Ok(());
                }
                found => found?,
            }
        };
        for row in found {
            let (Ok(day), Some(campaign_id)) = (
                NaiveDate::parse_from_str(&text(&row, "/segments/date"), "%Y-%m-%d"),
                resource_id(&row, "/campaign/id"),
            ) else {
                client
                    .record("campaign history: a row has no date or campaign id")
                    .await;
                return Err(Failure::Unavailable);
            };
            days.push(crate::workspace::history::CampaignDay {
                day,
                campaign_id,
                device: device_number(&text(&row, "/segments/device")),
                counts: counts(&row)?,
            });
        }
        let saved = store
            .save_history(
                id,
                from,
                to,
                if index == 0 { &campaigns } else { &[] },
                &days,
                if to == end { &changes } else { &[] },
            )
            .await;
        if let Err(error) = saved {
            let kind = error.to_string().chars().take(160).collect::<String>();
            tracing::warn!(kind, "Account history could not be stored");
            client
                .record(&format!("campaign history: not stored ({kind})"))
                .await;
            return Err(Failure::Unavailable);
        }
        if index == 0 {
            tracing::info!(ad_account = id, new_campaigns, "Campaign discovery stored");
        }
    }
    breakdowns::collect(client, store, id, end, recent_only).await;
    Ok(())
}

pub async fn collect_history(
    settings: &Settings,
    store: &WorkspaceStore<'_>,
    id: &str,
) -> Result<()> {
    collect(settings, store, id, false).await
}

pub async fn collect_recent_history(
    settings: &Settings,
    store: &WorkspaceStore<'_>,
    id: &str,
) -> Result<()> {
    collect(settings, store, id, true).await
}

async fn collect(
    settings: &Settings,
    store: &WorkspaceStore<'_>,
    id: &str,
    recent_only: bool,
) -> Result<()> {
    let outcome = match Client::connect(settings, store, id).await {
        Ok(client) => history(&client, settings, store, id, recent_only).await,
        Err(failure) => Err(failure),
    };
    if let Err(failure) = &outcome {
        store.history_failed(id, failure.message()).await?;
    }
    outcome.map_err(Error::from)
}

pub(crate) fn numeric(value: &Value) -> std::result::Result<f64, Failure> {
    let number = if value.is_null() {
        0.0
    } else {
        value
            .as_f64()
            .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
            .ok_or(Failure::Unavailable)?
    };
    if number.is_finite() {
        Ok(number)
    } else {
        Err(Failure::Unavailable)
    }
}

pub(crate) fn counts(row: &Value) -> std::result::Result<Counts, Failure> {
    let m = &row["metrics"];
    Ok(Counts {
        impressions: numeric(&m["impressions"])?,
        clicks: numeric(&m["clicks"])?,
        conversions: numeric(&m["conversions"])?,
        cost: numeric(&m["costMicros"])? / 1_000_000.0,
        value: numeric(&m["conversionsValue"])?,
    })
}

pub(crate) fn text(row: &Value, pointer: &str) -> String {
    row.pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or("Not available")
        .to_owned()
}

fn money(value: Option<f64>, currency: &str) -> String {
    value.map_or_else(
        || "Not available".into(),
        |value| {
            format!(
                "{}{}",
                english_currency_prefix(currency),
                english_number(value, 2)
            )
        },
    )
}
