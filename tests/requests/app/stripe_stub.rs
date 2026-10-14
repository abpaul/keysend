use std::collections::HashMap;

use axum::{
    extract::{Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};

use super::*;

#[derive(Default)]
pub(super) struct Calls {
    pub requests: Vec<(String, HashMap<String, String>)>,
    pub headers: Vec<HeaderMap>,
    pub sessions: HashMap<String, Value>,
    pub subscriptions: HashMap<String, Value>,
    pub fail: bool,
    pub unprepared: bool,
    pub tax_pending: bool,
    pub live: bool,
}

impl Calls {
    pub fn posted(&self, path: &str) -> Vec<&HashMap<String, String>> {
        self.requests
            .iter()
            .filter(|(name, _)| name == path)
            .map(|(_, form)| form)
            .collect()
    }

    fn has(&self, path: &str) -> bool {
        !self.unprepared || !self.posted(path).is_empty()
    }
}

fn list(found: bool, value: Value) -> Response {
    Json(json!({"data": if found { vec![value] } else { vec![] }})).into_response()
}

async fn tax_settings(
    State(calls): State<Arc<Mutex<super::Calls>>>,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let mut calls = calls.lock().unwrap();
    calls.stripe.requests.push(("tax/settings".into(), form));
    Json(json!({"status": if calls.stripe.tax_pending { "pending" } else { "active" }}))
        .into_response()
}

async fn products(State(calls): State<Arc<Mutex<super::Calls>>>) -> Response {
    list(
        calls.lock().unwrap().stripe.has("products"),
        json!({"id":"keysend_subscription"}),
    )
}

async fn prices(State(calls): State<Arc<Mutex<super::Calls>>>) -> Response {
    let found = calls.lock().unwrap().stripe.has("prices");
    list(
        found,
        json!({"id":"price_fixture","livemode":calls.lock().unwrap().stripe.live,"unit_amount":4900,"currency":"usd","recurring":{"interval":"month","interval_count":1}}),
    )
}

async fn portal_configurations(State(calls): State<Arc<Mutex<super::Calls>>>) -> Response {
    list(
        calls
            .lock()
            .unwrap()
            .stripe
            .has("billing_portal/configurations"),
        json!({"id":"bpc_fixture","is_default":true}),
    )
}

async fn webhook_endpoints(State(calls): State<Arc<Mutex<super::Calls>>>) -> Response {
    let calls = calls.lock().unwrap();
    let created = calls.stripe.posted("webhook_endpoints");
    list(
        !created.is_empty(),
        json!({"id":"we_fixture","url":created.first().map(|form| form["url"].clone()),"api_version":created.first().map(|form| form["api_version"].clone())}),
    )
}

async fn create(
    State(calls): State<Arc<Mutex<super::Calls>>>,
    uri: axum::http::Uri,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let path = uri.path().trim_start_matches("/stripe/").to_owned();
    let mut calls = calls.lock().unwrap();
    calls.stripe.requests.push((path, form));
    Json(json!({"livemode":calls.stripe.live})).into_response()
}

async fn customer(
    State(calls): State<Arc<Mutex<super::Calls>>>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let mut calls = calls.lock().unwrap();
    calls.stripe.headers.push(headers);
    calls.stripe.requests.push(("customers".into(), form));
    if calls.stripe.fail {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    Json(json!({"id":"cus_fixture","livemode":calls.stripe.live})).into_response()
}

async fn checkout(
    State(calls): State<Arc<Mutex<super::Calls>>>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let mut calls = calls.lock().unwrap();
    calls.stripe.headers.push(headers);
    calls
        .stripe
        .requests
        .push(("checkout/sessions".into(), form.clone()));
    if calls.stripe.fail {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let mode = if calls.stripe.live { "live" } else { "test" };
    let id = format!("cs_{mode}_fixture{}", calls.stripe.sessions.len() + 1);
    let value = json!({"id":id,"livemode":calls.stripe.live,"mode":form["mode"],"customer":form["customer"],"metadata":{"workspace_id":form["metadata[workspace_id]"]},"amount_subtotal":4900,"amount_total":5831,"currency":"usd","payment_status":"unpaid","subscription":null,"url":format!("https://checkout.stripe.com/c/pay/{id}")});
    calls.stripe.sessions.insert(id, value.clone());
    Json(value).into_response()
}

async fn session(
    State(calls): State<Arc<Mutex<super::Calls>>>,
    Path(id): Path<String>,
) -> Response {
    let calls = calls.lock().unwrap();
    if calls.stripe.fail {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    calls
        .stripe
        .sessions
        .get(&id)
        .map(|value| Json(value.clone()).into_response())
        .unwrap_or_else(|| StatusCode::NOT_FOUND.into_response())
}

async fn subscription(
    State(calls): State<Arc<Mutex<super::Calls>>>,
    Path(id): Path<String>,
) -> Response {
    let calls = calls.lock().unwrap();
    if calls.stripe.fail {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    calls
        .stripe
        .subscriptions
        .get(&id)
        .map(|value| Json(value.clone()).into_response())
        .unwrap_or_else(|| StatusCode::NOT_FOUND.into_response())
}

async fn portal(
    State(calls): State<Arc<Mutex<super::Calls>>>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let mut calls = calls.lock().unwrap();
    calls.stripe.headers.push(headers);
    calls
        .stripe
        .requests
        .push(("billing_portal/sessions".into(), form));
    Json(json!({"url":"https://billing.stripe.com/p/session/test_fixture"})).into_response()
}

async fn delete_customer(
    State(calls): State<Arc<Mutex<super::Calls>>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let mut calls = calls.lock().unwrap();
    calls.stripe.headers.push(headers);
    calls
        .stripe
        .requests
        .push((format!("DELETE customers/{id}"), HashMap::new()));
    Json(json!({"id":id,"deleted":true})).into_response()
}

pub(super) fn routes(calls: Arc<Mutex<super::Calls>>) -> Router {
    Router::new()
        .route("/stripe/customers/{id}", delete(delete_customer))
        .route("/stripe/customers", post(customer))
        .route("/stripe/checkout/sessions", post(checkout))
        .route("/stripe/checkout/sessions/{id}", get(session))
        .route("/stripe/subscriptions/{id}", get(subscription))
        .route("/stripe/billing_portal/sessions", post(portal))
        .route("/stripe/tax/settings", post(tax_settings))
        .route("/stripe/products", get(products).post(create))
        .route("/stripe/prices", get(prices).post(create))
        .route(
            "/stripe/billing_portal/configurations",
            get(portal_configurations).post(create),
        )
        .route("/stripe/billing_portal/configurations/{id}", post(create))
        .route(
            "/stripe/webhook_endpoints",
            get(webhook_endpoints).post(create),
        )
        .route("/stripe/webhook_endpoints/{id}", post(create))
        .with_state(calls)
}
