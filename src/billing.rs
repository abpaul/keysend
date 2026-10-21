use hmac::{Hmac, Mac};
use loco_rs::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Sha256;

use crate::settings::Settings;

pub mod setup;

pub const SUBSCRIPTION_USD_CENTS: i64 = 4_900;
pub const PRICE_LOOKUP_KEY: &str = "keysend_monthly";
pub const API_VERSION: &str = "2025-03-31.basil";

pub struct Checkout {
    pub id: String,
    pub amount: i64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionStatus {
    Active,
    Trialing,
    PastDue,
    Canceled,
    Unpaid,
    Incomplete,
    IncompleteExpired,
    Paused,
}

impl SubscriptionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Trialing => "trialing",
            Self::PastDue => "past_due",
            Self::Canceled => "canceled",
            Self::Unpaid => "unpaid",
            Self::Incomplete => "incomplete",
            Self::IncompleteExpired => "incomplete_expired",
            Self::Paused => "paused",
        }
    }
}

impl std::str::FromStr for SubscriptionStatus {
    type Err = ();

    fn from_str(text: &str) -> std::result::Result<Self, ()> {
        serde_json::from_value(Value::String(text.into())).map_err(|_| ())
    }
}

#[derive(Serialize)]
pub struct Subscription {
    pub id: String,
    pub created_at: i64,
    pub status: SubscriptionStatus,
    pub period_start: i64,
    pub period_end: i64,
    pub cancel_at_period_end: bool,
    pub canceled_at: Option<i64>,
}

pub struct VerifiedEvent {
    body: Value,
}

impl VerifiedEvent {
    pub fn id(&self) -> &str {
        self.body["id"].as_str().unwrap_or_default()
    }
    pub fn created(&self) -> i64 {
        self.body["created"].as_i64().unwrap_or_default()
    }
    pub fn kind(&self) -> &str {
        self.body["type"].as_str().unwrap_or_default()
    }
    pub fn object(&self) -> &Value {
        &self.body["data"]["object"]
    }
}

pub fn key_mode(key: &str) -> Option<bool> {
    match key.get(..8)? {
        "sk_test_" | "rk_test_" => Some(false),
        "sk_live_" | "rk_live_" => Some(true),
        _ => None,
    }
}

pub fn checkout_prefix(settings: &Settings) -> &'static str {
    if settings.stripe_live {
        "cs_live_"
    } else {
        "cs_test_"
    }
}

pub fn verify(secret: &str, signature: &str, bytes: &[u8], live: bool) -> Option<VerifiedEvent> {
    let timestamps = signature
        .split(',')
        .filter_map(|part| part.strip_prefix("t="))
        .collect::<Vec<_>>();
    if timestamps.len() != 1 {
        return None;
    }
    let timestamp = timestamps[0].parse::<i64>().ok()?;
    if crate::settings::wall_clock()
        .timestamp()
        .abs_diff(timestamp)
        > 300
    {
        return None;
    }
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(timestamps[0].as_bytes());
    mac.update(b".");
    mac.update(bytes);
    let matches = signature
        .split(',')
        .filter_map(|part| part.strip_prefix("v1="))
        .any(|value| {
            if value.len() != 64 || !value.is_ascii() {
                return false;
            }
            let decoded = value
                .as_bytes()
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| {
                    std::str::from_utf8(pair)
                        .ok()
                        .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                })
                .collect::<Option<Vec<_>>>();
            decoded.is_some_and(|decoded| mac.clone().verify_slice(&decoded).is_ok())
        });
    if !matches {
        return None;
    }
    let body = serde_json::from_slice::<Value>(bytes).ok()?;
    if body["livemode"] != live
        || !stripe_id(body["id"].as_str()?, "evt_")
        || body["created"].as_i64().is_none()
    {
        return None;
    }
    Some(VerifiedEvent { body })
}

pub fn stripe_id(value: &str, prefix: &str) -> bool {
    value.starts_with(prefix)
        && value.len() <= 200
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

pub fn unavailable() -> Error {
    Error::Message("Stripe is unavailable. Try again later.".into())
}

pub async fn request(
    settings: &Settings,
    method: reqwest::Method,
    path: &str,
    form: &[(String, String)],
    idempotency: Option<&str>,
) -> Result<Value> {
    let key = settings.stripe_key.as_deref().ok_or_else(unavailable)?;
    let mut request = settings
        .http
        .request(
            method,
            format!(
                "{}/{}",
                settings.billing.api_base.trim_end_matches('/'),
                path
            ),
        )
        .bearer_auth(key)
        .header("Stripe-Version", API_VERSION);
    if let Some(key) = idempotency {
        request = request.header("Idempotency-Key", key);
    }
    if !form.is_empty() {
        request = request.form(form);
    }
    let resource = path.split(['/', '?']).next().unwrap_or_default();
    let response = request.send().await.map_err(|error| {
        tracing::warn!(
            resource,
            kind = crate::settings::transport_kind(&error),
            "Stripe request failed"
        );
        unavailable()
    })?;
    if !response.status().is_success() {
        tracing::warn!(
            resource,
            http_status = response.status().as_u16(),
            "Stripe request failed"
        );
        return Err(unavailable());
    }
    response.json::<Value>().await.map_err(|error| {
        tracing::warn!(
            resource,
            kind = crate::settings::transport_kind(&error),
            "Stripe response could not be read"
        );
        unavailable()
    })
}

pub fn hosted_url(value: &Value, portal: bool) -> Result<String> {
    let url = value["url"].as_str().ok_or_else(unavailable)?;
    let parsed = reqwest::Url::parse(url).map_err(|_| unavailable())?;
    if parsed.scheme() != "https"
        || parsed.host_str()
            != Some(if portal {
                "billing.stripe.com"
            } else {
                "checkout.stripe.com"
            })
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(unavailable());
    }
    Ok(url.into())
}

fn subscription_price(price: &Value) -> bool {
    price["unit_amount"] == SUBSCRIPTION_USD_CENTS
        && price["currency"] == "usd"
        && price["recurring"]["interval"] == "month"
        && price["recurring"]["interval_count"] == 1
}

pub async fn catalog_price(settings: &Settings) -> Result<Option<String>> {
    let prices = request(
        settings,
        reqwest::Method::GET,
        &format!("prices?lookup_keys[]={PRICE_LOOKUP_KEY}&active=true&limit=1"),
        &[],
        None,
    )
    .await?;
    let Some(price) = prices["data"].as_array().and_then(|prices| prices.first()) else {
        return Ok(None);
    };
    let id = price["id"]
        .as_str()
        .filter(|id| stripe_id(id, "price_"))
        .ok_or_else(unavailable)?;
    if price["livemode"] != settings.stripe_live || !subscription_price(price) {
        tracing::warn!("The Stripe price does not match the Subscription price");
        return Err(unavailable());
    }
    Ok(Some(id.into()))
}

pub fn subscription(
    value: &Value,
    workspace: &str,
    customer: &str,
    live: bool,
) -> Result<Subscription> {
    let id = value["id"]
        .as_str()
        .filter(|id| stripe_id(id, "sub_"))
        .ok_or_else(unavailable)?;
    let items = value["items"]["data"].as_array().ok_or_else(unavailable)?;
    if value["livemode"] != live
        || value["customer"] != customer
        || value["metadata"]["workspace_id"] != workspace
        || items.len() != 1
    {
        return Err(unavailable());
    }
    let item = &items[0];
    if item["quantity"] != 1 || !subscription_price(&item["price"]) {
        return Err(unavailable());
    }
    Ok(Subscription {
        cancel_at_period_end: value["cancel_at_period_end"].as_bool().unwrap_or(false),
        canceled_at: value["canceled_at"].as_i64(),
        id: id.into(),
        created_at: value["created"].as_i64().ok_or_else(unavailable)?,
        status: value["status"]
            .as_str()
            .and_then(|status| status.parse().ok())
            .ok_or_else(|| {
                tracing::warn!("Stripe sent a subscription status that KeySend does not know");
                unavailable()
            })?,
        period_start: item["current_period_start"]
            .as_i64()
            .ok_or_else(unavailable)?,
        period_end: item["current_period_end"]
            .as_i64()
            .ok_or_else(unavailable)?,
    })
}

#[cfg(test)]
mod tests {
    use super::key_mode;

    #[test]
    fn the_key_prefix_selects_the_stripe_mode() {
        assert_eq!(key_mode("sk_test_abc"), Some(false));
        assert_eq!(key_mode("rk_test_abc"), Some(false));
        assert_eq!(key_mode("sk_live_abc"), Some(true));
        assert_eq!(key_mode("rk_live_abc"), Some(true));
        assert_eq!(key_mode("pk_live_abc"), None);
        assert_eq!(key_mode("whsec_abc"), None);
        assert_eq!(key_mode("sk_"), None);
    }
}
