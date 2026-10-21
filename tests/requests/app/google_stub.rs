use axum::{
    extract::{Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use std::collections::HashMap;

use super::*;

#[derive(Default)]
pub(super) struct Calls {
    pub monitoring_days: Option<Vec<Value>>,
    pub monitoring_campaigns: Option<Vec<Value>>,
    pub monitoring_audit_rows: HashMap<String, Vec<Value>>,
    pub monitoring_changes: Vec<Value>,
    pub monitoring_zone: Option<String>,
    pub breakdown_rows: HashMap<&'static str, Vec<Value>>,
    pub refused_query: Option<&'static str>,
    pub requests: Vec<(HeaderMap, Value)>,
    pub refresh: Vec<HashMap<String, String>>,
    pub revocations: Vec<HashMap<String, String>>,
    pub revoke_failure: bool,
    pub revoked: bool,
    pub disabled: bool,
    pub failed_status: Option<StatusCode>,
    pub failed_body: Option<Value>,
    pub failed_query: Option<&'static str>,
    /// The live budget of Campaign 701 for Optimize, and the changes sent to Google Ads.
    pub live_budget: Option<i64>,
    pub mutations: Vec<(String, HeaderMap, Value)>,
    /// The status of the next change that is sent for real; it is not applied.
    pub mutate_failure: Option<StatusCode>,
}

async fn token(
    State(calls): State<Arc<Mutex<super::Calls>>>,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let mut calls = calls.lock().unwrap();
    calls.google.refresh.push(form);
    if calls.google.revoked {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"invalid_grant"})),
        )
            .into_response();
    }
    Json(json!({"access_token":"google-access-secret"})).into_response()
}

const BREAKDOWNS: [&str; 9] = [
    "metrics.search_impression_share",
    "segments.conversion_action_name",
    "segments.ad_network_type",
    "segments.hour",
    "FROM geographic_view",
    "FROM geo_target_constant",
    "FROM age_range_view",
    "FROM gender_view",
    "segments.conversion_lag_bucket",
];

fn metrics() -> Value {
    json!({"impressions":"10000","clicks":"500","conversions":50.5,"costMicros":"2500000000","conversionsValue":10100.0})
}

async fn search(
    State(calls): State<Arc<Mutex<super::Calls>>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    assert_eq!(id, "4829153307");
    let mut calls = calls.lock().unwrap();
    calls.google.requests.push((headers, body.clone()));
    if let Some(status) = calls.google.failed_status {
        if let Some(body) = &calls.google.failed_body {
            return (status, Json(body.clone())).into_response();
        }
        return (status,Json(json!({"error":{"details":[{"errors":[{"errorCode":{"quotaError":"RESOURCE_EXHAUSTED"}}]}]}}))).into_response();
    }
    let query = body["query"].as_str().unwrap();
    if calls
        .google
        .failed_query
        .is_some_and(|part| query.contains(part))
    {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":{}}))).into_response();
    }
    if calls
        .google
        .refused_query
        .is_some_and(|part| query.contains(part))
    {
        return (StatusCode::BAD_REQUEST, Json(json!({"error":{"details":[{"errors":[{"errorCode":{"queryError":"PROHIBITED_FIELD_IN_SELECT_CLAUSE"}}]}]}}))).into_response();
    }
    for part in BREAKDOWNS {
        if !query.contains(part) {
            continue;
        }
        let range = query
            .split("segments.date BETWEEN '")
            .nth(1)
            .map(|range| (&range[..10], &range[17..27]));
        let rows = calls
            .google
            .breakdown_rows
            .get(part)
            .into_iter()
            .flatten()
            .filter(|row| match (row["segments"]["date"].as_str(), range) {
                (Some(day), Some((from, to))) => day >= from && day <= to,
                _ => true,
            })
            .cloned()
            .collect::<Vec<_>>();
        return Json(json!({"results":rows})).into_response();
    }
    if query.contains("customer.currency_code") {
        return Json(json!({"results":[{"customer":{"currencyCode":"EUR","timeZone":calls.google.monitoring_zone.as_deref().unwrap_or("Pacific/Kiritimati"),"status":if calls.google.disabled {"CANCELED"} else {"ENABLED"}}}]})).into_response();
    }
    if let Some(days) = &calls.google.monitoring_days {
        if query.contains("segments.date, segments.device") {
            let range = query.split("segments.date BETWEEN '").nth(1).unwrap();
            let from = &range[..10];
            let to = &range[17..27];
            let rows = days
                .iter()
                .filter(|row| {
                    let day = row["segments"]["date"].as_str().unwrap();
                    day >= from && day <= to
                })
                .cloned()
                .collect::<Vec<_>>();
            return Json(json!({"results":rows})).into_response();
        }
        if query.contains("FROM change_event") {
            return Json(json!({"results":calls.google.monitoring_changes})).into_response();
        }
    }
    for (resource, rows) in &calls.google.monitoring_audit_rows {
        if query.contains(&format!("FROM {resource} ")) {
            return Json(json!({"results":rows})).into_response();
        }
    }
    if let (true, Some(budget)) = (
        query.contains("WHERE campaign.id = 701"),
        calls.google.live_budget,
    ) {
        return Json(json!({"results":[{"campaign":{"id":"701","status":"ENABLED","biddingStrategyType":"MAXIMIZE_CONVERSIONS"},"campaignBudget":{"id":"9001","amountMicros":budget.to_string(),"period":"DAILY","explicitlyShared":false,"referenceCount":"1"}}]})).into_response();
    }
    if query.contains("campaign_budget.amount_micros") {
        if let Some(rows) = &calls.google.monitoring_campaigns {
            return Json(json!({"results":rows})).into_response();
        }
    }
    let metrics = metrics();
    let mut row = if query.contains("FROM change_event") {
        json!({"changeEvent":{"changeDateTime":"2026-10-05 10:00:00","changeResourceType":"CAMPAIGN_BUDGET","resourceChangeOperation":"UPDATE","resourceName":"customers/4829153307/changeEvents/1","campaign":"customers/4829153307/campaigns/701","changedFields":"amountMicros","oldResource":{"campaignBudget":{"amountMicros":"200000000"}},"newResource":{"campaignBudget":{"amountMicros":"80000000"}}},"campaign":{"name":"Search - Real"}})
    } else if query.contains("campaign_budget.amount_micros") {
        json!({"campaign":{"id":"701","name":"Search - Real","status":"ENABLED","advertisingChannelType":"SEARCH","biddingStrategyType":"MAXIMIZE_CONVERSIONS"},"campaignBudget":{"amountMicros":"80000000"}})
    } else if query.contains("segments.date, segments.device") {
        json!({"campaign":{"id":"701"},"segments":{"date":if query.contains("AND '2026-10-") {"2026-09-15"} else {"2020-01-01"},"device":"MOBILE"},"metrics":metrics})
    } else if query.contains("FROM campaign") {
        json!({"campaign":{"name":"Search - Real"},"metrics":metrics})
    } else if query.contains("FROM keyword_view") {
        json!({"adGroupCriterion":{"keyword":{"text":"real dentist","matchType":"EXACT"},"qualityInfo":{"qualityScore":7}},"metrics":metrics})
    } else if query.contains("FROM conversion_action") {
        json!({"conversionAction":{"id":"9"}})
    } else if query.contains("FROM search_term_view") {
        json!({"searchTermView":{"searchTerm":"real dentist nearby"},"metrics":metrics})
    } else if query.contains("FROM ad_group_ad") {
        json!({"adGroupAd":{"ad":{"id":"123","type":"RESPONSIVE_SEARCH_AD","responsiveSearchAd":{"headlines":[{"text":"Book Your Appointment","pinnedField":"HEADLINE_1"},{"text":"Real Dentist Nearby"},{"text":"Open Six Days"},{"text":"Call Today"}]}},"adStrength":"GOOD"},"adGroup":{"name":"Dentists"},"campaign":{"name":"Search - Real"},"metrics":metrics})
    } else if query.contains("FROM landing_page_view") {
        json!({"landingPageView":{"unexpandedFinalUrl":"https://example.com/booking"},"metrics":metrics})
    } else {
        assert!(query.contains("FROM customer"), "Unexpected query: {query}");
        json!({"metrics":metrics})
    };
    if let Some(days) = &calls.google.monitoring_days {
        if query.contains("campaign.id") {
            row["campaign"]["id"] = json!("701");
            row["campaign"]["name"] = json!("Search - Real");
        }
        if let Some(range) = query.split("segments.date BETWEEN '").nth(1) {
            let from = &range[..10];
            let to = &range[17..27];
            let mut totals = keysend::monitoring::audit::Counts::default();
            for day in days.iter().filter(|row| {
                let day = row["segments"]["date"].as_str().unwrap();
                day >= from && day <= to
            }) {
                let value = |field: &str| {
                    day["metrics"][field]
                        .as_f64()
                        .unwrap_or_else(|| day["metrics"][field].as_str().unwrap().parse().unwrap())
                };
                totals.impressions += value("impressions");
                totals.clicks += value("clicks");
                totals.cost += value("costMicros");
                totals.conversions += value("conversions");
                totals.value += value("conversionsValue");
            }
            row["metrics"] = json!({"impressions":totals.impressions.to_string(),"clicks":totals.clicks.to_string(),"costMicros":totals.cost.to_string(),"conversions":totals.conversions,"conversionsValue":totals.value});
        }
    }
    Json(json!({"results":[row]})).into_response()
}

async fn mutate_budgets(
    State(calls): State<Arc<Mutex<super::Calls>>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    assert_eq!(id, "4829153307");
    let mut calls = calls.lock().unwrap();
    calls
        .google
        .mutations
        .push(("campaignBudgets".into(), headers, body.clone()));
    let validate = body["validateOnly"].as_bool().unwrap();
    if !validate {
        if let Some(status) = calls.google.mutate_failure.take() {
            return (status, Json(json!({"error":{}}))).into_response();
        }
        let amount = body["operations"][0]["update"]["amountMicros"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        calls.google.live_budget = Some(amount);
    }
    (
        [("request-id", "stub-request-1")],
        Json(json!({"results":[{"resourceName":body["operations"][0]["update"]["resourceName"]}]})),
    )
        .into_response()
}

async fn revoke(
    State(calls): State<Arc<Mutex<super::Calls>>>,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let mut calls = calls.lock().unwrap();
    calls.google.revocations.push(form);
    if calls.google.revoke_failure {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    StatusCode::OK.into_response()
}

pub(super) fn routes(calls: Arc<Mutex<super::Calls>>) -> Router {
    Router::new()
        .route("/google/revoke", post(revoke))
        .route("/google/token", post(token))
        .route("/google/v25/customers/{id}/googleAds:search", post(search))
        .route(
            "/google/v25/customers/{id}/campaignBudgets:mutate",
            post(mutate_budgets),
        )
        .with_state(calls)
}
