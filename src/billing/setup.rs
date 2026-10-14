use async_trait::async_trait;
use loco_rs::{
    app::AppContext,
    task::{Task, TaskInfo, Vars},
    Error, Result,
};
use reqwest::Method;
use serde_json::Value;

use super::{
    catalog_price, request, stripe_id, unavailable, API_VERSION, PRICE_LOOKUP_KEY,
    SUBSCRIPTION_USD_CENTS,
};
use crate::settings::Settings;

const PRODUCT: &str = "keysend_subscription";
const SAAS_FOR_BUSINESS_TAX_CODE: &str = "txcd_10103001";
const WEBHOOK_EVENTS: [&str; 5] = [
    "checkout.session.completed",
    "checkout.session.async_payment_succeeded",
    "customer.subscription.created",
    "customer.subscription.updated",
    "customer.subscription.deleted",
];

pub struct StripeSetup;

#[async_trait]
impl Task for StripeSetup {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "stripe_setup".into(),
            detail: "Prepare the Stripe account: Tax defaults, the Subscription price, the Subscription portal and the webhook".into(),
        }
    }

    async fn run(&self, ctx: &AppContext, _vars: &Vars) -> Result<()> {
        for line in prepare(&Settings::load(ctx)?).await? {
            println!("{line}");
        }
        Ok(())
    }
}

fn form(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(name, value)| ((*name).into(), (*value).into()))
        .collect()
}

fn first(list: &Value) -> Option<&Value> {
    list["data"].as_array().and_then(|data| data.first())
}

pub async fn prepare(settings: &Settings) -> Result<Vec<String>> {
    let mut report = Vec::new();

    let tax = request(
        settings,
        Method::POST,
        "tax/settings",
        &form(&[
            ("defaults[tax_code]", SAAS_FOR_BUSINESS_TAX_CODE),
            ("defaults[tax_behavior]", "exclusive"),
        ]),
        None,
    )
    .await?;
    if tax["status"] != "active" {
        return Err(Error::Message(
            "Stripe Tax is not active. Give Stripe the head office address in the Tax settings, then run this task again.".into(),
        ));
    }
    report.push("Stripe Tax is active.".into());

    let products = request(
        settings,
        Method::GET,
        &format!("products?ids[]={PRODUCT}"),
        &[],
        None,
    )
    .await?;
    if first(&products).is_none() {
        request(
            settings,
            Method::POST,
            "products",
            &form(&[
                ("id", PRODUCT),
                ("name", "KeySend Subscription"),
                ("tax_code", SAAS_FOR_BUSINESS_TAX_CODE),
            ]),
            None,
        )
        .await?;
        report.push(format!("Created the product {PRODUCT}."));
    }

    if catalog_price(settings).await?.is_none() {
        request(
            settings,
            Method::POST,
            "prices",
            &form(&[
                ("product", PRODUCT),
                ("lookup_key", PRICE_LOOKUP_KEY),
                ("currency", "usd"),
                ("unit_amount", &SUBSCRIPTION_USD_CENTS.to_string()),
                ("recurring[interval]", "month"),
                ("tax_behavior", "exclusive"),
            ]),
            None,
        )
        .await?;
        report.push(format!("Created the price {PRICE_LOOKUP_KEY}."));
    }

    let portals = request(
        settings,
        Method::GET,
        "billing_portal/configurations?is_default=true&limit=1",
        &[],
        None,
    )
    .await?;
    let portal = match first(&portals) {
        Some(portal) => format!(
            "billing_portal/configurations/{}",
            portal["id"]
                .as_str()
                .filter(|id| stripe_id(id, "bpc_"))
                .ok_or_else(unavailable)?
        ),
        None => "billing_portal/configurations".into(),
    };
    request(
        settings,
        Method::POST,
        &portal,
        &form(&[
            ("features[invoice_history][enabled]", "true"),
            ("features[payment_method_update][enabled]", "true"),
            ("features[customer_update][enabled]", "true"),
            ("features[customer_update][allowed_updates][0]", "name"),
            ("features[customer_update][allowed_updates][1]", "address"),
            ("features[customer_update][allowed_updates][2]", "tax_id"),
            ("features[subscription_cancel][enabled]", "true"),
            ("features[subscription_cancel][mode]", "at_period_end"),
        ]),
        None,
    )
    .await?;
    report.push(
        "The Subscription portal shows invoices and lets a User change the card, the billing details and the tax ID, and cancel at the end of the period.".into(),
    );

    if !settings.secure {
        report.push(format!(
            "No webhook was created: Stripe cannot reach {}.",
            settings.origin
        ));
        return Ok(report);
    }
    let url = format!("{}/stripe/webhook", settings.origin);
    let endpoints = request(
        settings,
        Method::GET,
        "webhook_endpoints?limit=100",
        &[],
        None,
    )
    .await?;
    let known = endpoints["data"]
        .as_array()
        .and_then(|endpoints| endpoints.iter().find(|endpoint| endpoint["url"] == url));
    let mut webhook = vec![];
    for (index, event) in WEBHOOK_EVENTS.iter().enumerate() {
        webhook.push((format!("enabled_events[{index}]"), (*event).to_owned()));
    }
    match known {
        Some(endpoint) => {
            if endpoint["api_version"] != API_VERSION {
                return Err(Error::Message(format!(
                    "The Stripe webhook for {url} does not use API version {API_VERSION}. Delete it in Stripe, then run this task again."
                )));
            }
            let id = endpoint["id"]
                .as_str()
                .filter(|id| stripe_id(id, "we_"))
                .ok_or_else(unavailable)?;
            request(
                settings,
                Method::POST,
                &format!("webhook_endpoints/{id}"),
                &webhook,
                None,
            )
            .await?;
            report.push(format!("The webhook for {url} has the events of KeySend."));
        }
        None => {
            webhook.push(("url".into(), url.clone()));
            webhook.push(("api_version".into(), API_VERSION.into()));
            request(settings, Method::POST, "webhook_endpoints", &webhook, None).await?;
            report.push(format!(
                "Created the webhook for {url}. Copy its signing secret from Stripe to STRIPE_WEBHOOK_SECRET."
            ));
        }
    }
    Ok(report)
}
