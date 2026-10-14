use std::sync::{Arc, OnceLock};

use axum::{
    body::Bytes,
    extract::Query,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect},
    Extension,
};
use loco_rs::prelude::*;
use reqwest::Method;
use tokio::sync::Mutex;

use crate::{
    auth::Authenticated,
    billing::{self, Checkout},
    language::Text,
    monitoring::access,
    settings::Settings,
    workspace::{Workspace, WorkspaceStore},
};

#[derive(serde::Deserialize)]
struct Returned {
    checkout: Option<String>,
}

fn day_month(text: &Text, timestamp: i64) -> String {
    chrono::DateTime::from_timestamp(timestamp, 0)
        .map(|date| text.written.format.day_month(date.date_naive()))
        .unwrap_or_default()
}

async fn show(
    Authenticated(signed): Authenticated,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Extension(settings): Extension<Arc<Settings>>,
    Query(query): Query<Returned>,
) -> Result<Response> {
    let text = signed.user.language.text()?;
    let store = WorkspaceStore::new(&ctx.db, &signed.workspace);
    let now = settings.now().timestamp();
    let subscription = store.subscription().await?;
    let started = store.trial_started_at().await?;
    let trial_end = started + access::TRIAL_SECONDS;
    let ad_accounts = store.ad_accounts().await?.len();
    let state = if access::subscription_open(subscription.as_ref(), now) {
        "active"
    } else if access::payment_due(subscription.as_ref()) {
        "payment_due"
    } else if access::trial_open(started, now) {
        "trial"
    } else {
        "ended"
    };
    format::render().view(
        &v,
        "subscription/index.html",
        data!({
            "state": state,
            "price": text.whole_money((crate::billing::SUBSCRIPTION_USD_CENTS / 100) as f64, "USD"),
            "ad_accounts": ad_accounts,
            "ad_account_limit": access::AD_ACCOUNT_LIMIT,
            "over_limit": ad_accounts > access::AD_ACCOUNT_LIMIT,
            "excess": ad_accounts.saturating_sub(access::AD_ACCOUNT_LIMIT),
            "days_left": (trial_end - now + 86_399) / 86_400,
            "trial_ends_on": day_month(text, trial_end),
            "had_subscription": subscription.is_some(),
            "next_payment_on": subscription.as_ref().map(|subscription| day_month(text, subscription.period_end)),
            "started_on": subscription.as_ref().and_then(|subscription| chrono::DateTime::from_timestamp(subscription.created_at, 0)).map(|date| text.written.format.date_with_year(date.date_naive())),
            "ready": settings.payments_ready(),
            "returned": query.checkout.as_deref() == Some("returned") && state != "active",
        }),
    )
}

async fn billing_moved() -> Redirect {
    Redirect::to("/subscription")
}

async fn subscribe(
    Authenticated(signed): Authenticated,
    State(ctx): State<AppContext>,
    Extension(settings): Extension<Arc<Settings>>,
) -> Result<Response> {
    let text = signed.user.language.text()?;
    let store = WorkspaceStore::new(&ctx.db, &signed.workspace);
    if store.subscription_active().await?
        || access::payment_due(store.subscription().await?.as_ref())
    {
        return Ok(Redirect::to("/subscription").into_response());
    }
    if store.ad_accounts().await?.len() > access::AD_ACCOUNT_LIMIT {
        return Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            text.written.page_subscription_limit,
        )
            .into_response());
    }
    if !settings.payments_ready() {
        return Ok((
            StatusCode::SERVICE_UNAVAILABLE,
            text.written.page_payments_unavailable,
        )
            .into_response());
    }
    let result = async {
        let customer = if let Some(customer) = store.stripe_customer().await? {
            customer
        } else {
            let value = billing::request(
                &settings,
                Method::POST,
                "customers",
                &[
                    ("email".into(), signed.user.email.clone()),
                    (
                        "metadata[workspace_id]".into(),
                        signed.workspace.id().into(),
                    ),
                ],
                Some(&format!("keysend-customer-{}", signed.workspace.id())),
            )
            .await?;
            let customer = value["id"]
                .as_str()
                .filter(|id| billing::stripe_id(id, "cus_"))
                .ok_or_else(billing::unavailable)?;
            if value["livemode"] != settings.stripe_live {
                return Err(billing::unavailable());
            }
            store.save_stripe_customer(customer).await?;
            store
                .stripe_customer()
                .await?
                .ok_or_else(billing::unavailable)?
        };
        let price = billing::catalog_price(&settings)
            .await?
            .ok_or_else(billing::unavailable)?;
        let amount = crate::billing::SUBSCRIPTION_USD_CENTS;
        let form = vec![
            ("customer".into(), customer),
            ("mode".into(), "subscription".into()),
            (
                "success_url".into(),
                format!("{}/subscription?checkout=returned", settings.origin),
            ),
            (
                "cancel_url".into(),
                format!("{}/subscription", settings.origin),
            ),
            (
                "metadata[workspace_id]".into(),
                signed.workspace.id().into(),
            ),
            ("line_items[0][price]".into(), price),
            ("line_items[0][quantity]".into(), "1".into()),
            ("managed_payments[enabled]".into(), "true".into()),
            (
                "subscription_data[metadata][workspace_id]".into(),
                signed.workspace.id().into(),
            ),
        ];
        let value = billing::request(
            &settings,
            Method::POST,
            "checkout/sessions",
            &form,
            Some(&format!(
                "keysend-checkout-{}",
                crate::identity::random_token()
            )),
        )
        .await?;
        let id = value["id"]
            .as_str()
            .filter(|id| billing::stripe_id(id, billing::checkout_prefix(&settings)))
            .ok_or_else(billing::unavailable)?;
        if value["livemode"] != settings.stripe_live {
            return Err(billing::unavailable());
        }
        let url = billing::hosted_url(&value, false)?;
        store
            .save_checkout(&Checkout {
                id: id.into(),
                amount,
            })
            .await?;
        Ok(Redirect::to(&url).into_response())
    }
    .await;
    Ok(result.unwrap_or_else(|error| {
        tracing::warn!(error = %error, "Stripe checkout failed");
        (
            StatusCode::SERVICE_UNAVAILABLE,
            text.written.page_stripe_unavailable,
        )
            .into_response()
    }))
}

async fn portal(
    Authenticated(signed): Authenticated,
    State(ctx): State<AppContext>,
    Extension(settings): Extension<Arc<Settings>>,
) -> Result<Response> {
    let text = signed.user.language.text()?;
    let store = WorkspaceStore::new(&ctx.db, &signed.workspace);
    let Some(customer) = store.stripe_customer().await? else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let result = async {
        let value = billing::request(
            &settings,
            Method::POST,
            "billing_portal/sessions",
            &[
                ("customer".into(), customer),
                (
                    "return_url".into(),
                    format!("{}/subscription", settings.origin),
                ),
            ],
            None,
        )
        .await?;
        Ok::<_, Error>(Redirect::to(&billing::hosted_url(&value, true)?).into_response())
    }
    .await;
    Ok(result.unwrap_or_else(|error| {
        tracing::warn!(error = %error, "Stripe portal failed");
        (
            StatusCode::SERVICE_UNAVAILABLE,
            text.written.page_stripe_unavailable,
        )
            .into_response()
    }))
}

static EVENTS: OnceLock<Mutex<()>> = OnceLock::new();

async fn webhook(
    State(ctx): State<AppContext>,
    Extension(settings): Extension<Arc<Settings>>,
    headers: HeaderMap,
    bytes: Bytes,
) -> Result<Response> {
    let Some(secret) = settings.stripe_webhook.as_deref() else {
        return Ok(StatusCode::SERVICE_UNAVAILABLE.into_response());
    };
    let Some(signature) = headers
        .get("stripe-signature")
        .and_then(|value| value.to_str().ok())
    else {
        tracing::warn!("Stripe webhook refused: the signature header is missing");
        return Ok(StatusCode::BAD_REQUEST.into_response());
    };
    let Some(event) = billing::verify(secret, signature, &bytes, settings.stripe_live) else {
        tracing::warn!("Stripe webhook refused: the signature or the event is not valid");
        return Ok(StatusCode::BAD_REQUEST.into_response());
    };
    let object = event.object();
    let Some(workspace) = object["metadata"]["workspace_id"].as_str() else {
        return Ok(StatusCode::OK.into_response());
    };
    let workspace = Workspace::from_identity(workspace.into());
    let _guard = EVENTS.get_or_init(|| Mutex::new(())).lock().await;
    let store = WorkspaceStore::new(&ctx.db, &workspace);
    let Some(customer) = store.stripe_customer().await? else {
        return Ok(StatusCode::OK.into_response());
    };
    let result = async {
        match event.kind() {
            "checkout.session.completed" | "checkout.session.async_payment_succeeded" => {
                let Some(id) = object["id"]
                    .as_str()
                    .filter(|id| billing::stripe_id(id, billing::checkout_prefix(&settings)))
                else {
                    return Ok(());
                };
                let Some(checkout) = store.checkout(id).await? else {
                    return Ok(());
                };
                let session = billing::request(
                    &settings,
                    Method::GET,
                    &format!("checkout/sessions/{id}"),
                    &[],
                    None,
                )
                .await?;
                if session["id"] != id
                    || session["livemode"] != settings.stripe_live
                    || session["customer"] != customer
                    || session["metadata"]["workspace_id"] != workspace.id()
                    || session["payment_status"] != "paid"
                    || session["amount_subtotal"] != checkout.amount
                    || session["currency"] != "usd"
                {
                    return Ok(());
                }
                if session["mode"] != "subscription" {
                    return Ok(());
                }
                let id = session["subscription"]
                    .as_str()
                    .filter(|id| billing::stripe_id(id, "sub_"))
                    .ok_or_else(billing::unavailable)?;
                let value = billing::request(
                    &settings,
                    Method::GET,
                    &format!("subscriptions/{id}"),
                    &[],
                    None,
                )
                .await?;
                let subscription =
                    billing::subscription(&value, workspace.id(), &customer, settings.stripe_live)?;
                store
                    .apply_billing_event(&event, Some(&checkout), Some(&subscription))
                    .await?;
            }
            "customer.subscription.created"
            | "customer.subscription.updated"
            | "customer.subscription.deleted" => {
                let Some(id) = object["id"]
                    .as_str()
                    .filter(|id| billing::stripe_id(id, "sub_"))
                else {
                    return Ok(());
                };
                let Some(known) = store.subscription().await? else {
                    return Ok(());
                };
                if known.id != id {
                    return Ok(());
                }
                let value = billing::request(
                    &settings,
                    Method::GET,
                    &format!("subscriptions/{id}"),
                    &[],
                    None,
                )
                .await?;
                let subscription =
                    billing::subscription(&value, workspace.id(), &customer, settings.stripe_live)?;
                store
                    .apply_billing_event(&event, None, Some(&subscription))
                    .await?;
            }
            _ => {}
        }
        Ok::<_, Error>(())
    }
    .await;
    Ok(match result {
        Ok(()) => StatusCode::OK,
        Err(error) => {
            tracing::warn!(error = %error, event_type = event.kind(), "Stripe webhook failed");
            StatusCode::SERVICE_UNAVAILABLE
        }
    }
    .into_response())
}

pub fn routes() -> Routes {
    Routes::new()
        .add("/billing", get(billing_moved))
        .add("/subscription", get(show).post(subscribe))
        .add("/subscription/portal", post(portal))
        .add("/stripe/webhook", post(webhook))
}
