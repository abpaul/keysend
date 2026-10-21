use hmac::{Hmac, Mac};
use sha2::Sha256;

use super::*;

fn signature(body: &str, timestamp: i64) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(b"whsec_stub-stripe-secret").unwrap();
    mac.update(format!("{timestamp}.{body}").as_bytes());
    let hex = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("t={timestamp},v1={hex}")
}

impl Fixture {
    async fn event(
        &self,
        id: &str,
        kind: &str,
        object: Value,
        created: i64,
    ) -> axum_test::TestResponse {
        let live = self.calls.lock().unwrap().stripe.live;
        let body =
            json!({"id":id,"type":kind,"livemode":live,"created":created,"data":{"object":object}})
                .to_string();
        let response = self
            .request
            .post("/stripe/webhook")
            .add_header(
                "stripe-signature",
                signature(&body, chrono::Utc::now().timestamp()),
            )
            .text(body)
            .await;
        assert_eq!(response.header("cache-control"), "no-store");
        response
    }

    pub(in crate::requests) async fn subscribe(&self) -> Value {
        self.subscribe_as("sub_fixture", 1700000000).await
    }

    async fn subscribe_as(&self, subscription: &str, created: i64) -> Value {
        let response = self
            .request
            .post("/subscription")
            .add_header("origin", ORIGIN)
            .add_cookie(self.a.clone())
            .await;
        assert_eq!(response.status_code(), 303, "{}", response.text());
        let location = response.header("location");
        let id = location.to_str().unwrap().rsplit('/').next().unwrap();
        let value = {
            let mut calls = self.calls.lock().unwrap();
            let session = calls.stripe.sessions.get_mut(id).unwrap();
            session["payment_status"] = json!("paid");
            session["subscription"] = json!(subscription);
            let object = session.clone();
            let value = json!({"id":subscription,"created":created,"customer":"cus_fixture","livemode":calls.stripe.live,"metadata":object["metadata"],"status":"active","items":{"data":[{"quantity":1,"current_period_start":1700000000i64,"current_period_end":3000000000i64,"price":{"unit_amount":4900,"currency":"usd","recurring":{"interval":"month","interval_count":1}}}]}});
            calls
                .stripe
                .subscriptions
                .insert(subscription.into(), value);
            object
        };
        assert_eq!(
            self.event(
                &format!("evt_checkout_{subscription}"),
                "checkout.session.completed",
                value.clone(),
                chrono::Utc::now().timestamp()
            )
            .await
            .status_code(),
            200
        );
        value
    }
}

#[tokio::test]
#[serial]
async fn with_a_live_key_a_paid_checkout_opens_the_subscription_and_test_events_are_refused() {
    let f = Fixture::live_payments().await;
    let page = f.page("/subscription").await;
    assert!(
        page.contains(r#"<button class="button" type="submit">Start the Subscription</button>"#)
    );
    let checkout = f.subscribe().await;
    assert_eq!(checkout["id"], "cs_live_fixture1");
    assert_eq!(
        f.calls.lock().unwrap().stripe.headers[0]["authorization"],
        "Bearer rk_live_stub-stripe-secret"
    );
    let signed = signed(&f).await;
    assert!(store(&f, &signed).subscription_active().await.unwrap());
    let canceled = {
        let mut calls = f.calls.lock().unwrap();
        let value = calls.stripe.subscriptions.get_mut("sub_fixture").unwrap();
        value["status"] = json!("canceled");
        value.clone()
    };
    f.calls.lock().unwrap().stripe.live = false;
    assert_eq!(
        f.event(
            "evt_test_mode_cancel",
            "customer.subscription.deleted",
            canceled,
            1800000000
        )
        .await
        .status_code(),
        400
    );
    assert!(store(&f, &signed).subscription_active().await.unwrap());
}

fn store<'a>(f: &'a Fixture, signed: &'a identity::SignedIn) -> WorkspaceStore<'a> {
    WorkspaceStore::new(&f.ctx.db, &signed.workspace)
}

async fn signed(f: &Fixture) -> identity::SignedIn {
    identity::authenticate(&f.ctx.db, f.a.value())
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
#[serial]
async fn missing_payment_credentials_disable_checkout_without_a_stripe_call() {
    let f = Fixture::configured(false, false, false).await;
    let page = f.page("/subscription").await;
    assert!(page.contains("Payments are not available yet."));
    assert!(page.contains(
        r#"<button class="button" type="submit" disabled>Start the Subscription</button>"#
    ));
    let response = f
        .request
        .post("/subscription")
        .add_header("origin", ORIGIN)
        .add_cookie(f.a.clone())
        .await;
    assert_eq!(response.status_code(), 503);
    assert_eq!(response.text(), "Payments are not available yet.");
    assert!(f.calls.lock().unwrap().stripe.requests.is_empty());
}

#[tokio::test]
#[serial]
async fn a_subscription_ends_when_its_period_ends() {
    let f = Fixture::new(false).await;
    f.subscribe().await;
    let signed = signed(&f).await;
    assert!(store(&f, &signed).subscription_active().await.unwrap());
    let value = {
        let mut calls = f.calls.lock().unwrap();
        let value = calls.stripe.subscriptions.get_mut("sub_fixture").unwrap();
        value["items"]["data"][0]["current_period_end"] = json!(1700000001);
        value.clone()
    };
    assert_eq!(
        f.event(
            "evt_expired",
            "customer.subscription.updated",
            value,
            1800000000
        )
        .await
        .status_code(),
        200
    );
    assert!(!store(&f, &signed).subscription_active().await.unwrap());
}

#[tokio::test]
#[serial]
async fn late_checkout_for_old_subscription_does_not_replace_new_subscription() {
    let f = Fixture::new(false).await;
    let old = f.subscribe().await;
    let canceled = {
        let mut calls = f.calls.lock().unwrap();
        let value = calls.stripe.subscriptions.get_mut("sub_fixture").unwrap();
        value["status"] = json!("canceled");
        value.clone()
    };
    assert_eq!(
        f.event(
            "evt_cancel_old",
            "customer.subscription.deleted",
            canceled,
            1800000000
        )
        .await
        .status_code(),
        200
    );
    f.subscribe_as("sub_replacement", 1800000000).await;
    assert_eq!(
        f.event(
            "evt_late_old_checkout",
            "checkout.session.completed",
            old,
            1700000000
        )
        .await
        .status_code(),
        200
    );
    let signed = signed(&f).await;
    let subscription = store(&f, &signed).subscription().await.unwrap().unwrap();
    assert_eq!(subscription.id, "sub_replacement");
    assert_eq!(
        subscription.status,
        keysend::billing::SubscriptionStatus::Active
    );
}

#[tokio::test]
#[serial]
async fn invalid_expired_tampered_and_live_webhooks_are_refused() {
    let f = Fixture::new(false).await;
    for (body, header) in [
        ("{}".to_owned(), "t=1,v1=invalid".to_owned()),
        (
            "{}".to_owned(),
            signature("{}", chrono::Utc::now().timestamp() - 601),
        ),
        (
            "{\"tampered\":true}".to_owned(),
            signature("{}", chrono::Utc::now().timestamp()),
        ),
        (
            json!({"id":"evt_live","created":1700000000,"livemode":true}).to_string(),
            String::new(),
        ),
    ] {
        let header = if header.is_empty() {
            signature(&body, chrono::Utc::now().timestamp())
        } else {
            header
        };
        assert_eq!(
            f.request
                .post("/stripe/webhook")
                .add_header("stripe-signature", header)
                .text(body)
                .await
                .status_code(),
            400
        );
    }
    assert_eq!(
        f.request
            .post("/stripe/webhook")
            .text("{}")
            .await
            .status_code(),
        400
    );
    let signed = signed(&f).await;
    assert!(store(&f, &signed).subscription().await.unwrap().is_none());
}

#[tokio::test]
#[serial]
async fn subscription_events_use_current_state() {
    let f = Fixture::new(false).await;
    let value = f.subscribe().await;
    let response = f
        .request
        .post("/subscription/portal")
        .add_header("origin", ORIGIN)
        .add_cookie(f.a.clone())
        .await;
    assert_eq!(response.status_code(), 303);
    assert_eq!(
        response.header("location"),
        "https://billing.stripe.com/p/session/test_fixture"
    );
    f.calls
        .lock()
        .unwrap()
        .stripe
        .subscriptions
        .get_mut("sub_fixture")
        .unwrap()["status"] = json!("canceled");
    let mut object = f.calls.lock().unwrap().stripe.subscriptions["sub_fixture"].clone();
    assert_eq!(
        f.event(
            "evt_canceled",
            "customer.subscription.deleted",
            object.clone(),
            1800000000
        )
        .await
        .status_code(),
        200
    );
    object["status"] = json!("active");
    for _ in 0..2 {
        assert_eq!(
            f.event(
                "evt_old_active",
                "customer.subscription.updated",
                object.clone(),
                1700000000
            )
            .await
            .status_code(),
            200
        );
    }
    assert_eq!(
        f.event(
            "evt_subscription_checkout",
            "checkout.session.completed",
            value,
            1700000000
        )
        .await
        .status_code(),
        200
    );
    let signed = signed(&f).await;
    assert_eq!(
        store(&f, &signed)
            .subscription()
            .await
            .unwrap()
            .unwrap()
            .status,
        keysend::billing::SubscriptionStatus::Canceled
    );
    assert!(!store(&f, &signed).subscription_active().await.unwrap());
}

#[tokio::test]
#[serial]
async fn stripe_failures_retry_without_granting_access_and_wrong_amounts_are_refused() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let writer = LogCapture(captured.clone());
    let _subscriber = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish(),
    );
    let f = Fixture::new(false).await;
    f.calls.lock().unwrap().stripe.fail = true;
    assert_eq!(
        f.request
            .post("/subscription")
            .add_header("origin", ORIGIN)
            .add_cookie(f.a.clone())
            .await
            .status_code(),
        503
    );
    {
        let logs = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
        assert!(
            logs.contains("Stripe request failed") && logs.contains("http_status=429"),
            "{logs}"
        );
        assert!(logs.contains("Stripe checkout failed"), "{logs}");
        assert!(!logs.contains("sk_test_stub-stripe-secret"));
    }
    f.calls.lock().unwrap().stripe.fail = false;
    let response = f
        .request
        .post("/subscription")
        .add_header("origin", ORIGIN)
        .add_cookie(f.a.clone())
        .await;
    assert_eq!(response.status_code(), 303);
    let mut object = {
        let mut calls = f.calls.lock().unwrap();
        let value = calls.stripe.sessions.get_mut("cs_test_fixture1").unwrap();
        value["payment_status"] = json!("paid");
        value["subscription"] = json!("sub_retry");
        value["amount_subtotal"] = json!(1);
        let object = value.clone();
        calls.stripe.subscriptions.insert(
            "sub_retry".into(),
            json!({"id":"sub_retry","created":1700000000i64,"customer":"cus_fixture","livemode":false,"metadata":object["metadata"],"status":"active","items":{"data":[{"quantity":1,"current_period_start":1700000000i64,"current_period_end":3000000000i64,"price":{"unit_amount":4900,"currency":"usd","recurring":{"interval":"month","interval_count":1}}}]}}),
        );
        object
    };
    assert_eq!(
        f.event(
            "evt_wrong_price",
            "checkout.session.completed",
            object.clone(),
            1700000000
        )
        .await
        .status_code(),
        200
    );
    let signed = signed(&f).await;
    assert!(!store(&f, &signed).subscription_active().await.unwrap());
    object["amount_subtotal"] = json!(4900);
    {
        let mut calls = f.calls.lock().unwrap();
        calls
            .stripe
            .sessions
            .insert("cs_test_fixture1".into(), object.clone());
        calls.stripe.fail = true;
    }
    assert_eq!(
        f.event(
            "evt_retry_paid",
            "checkout.session.completed",
            object.clone(),
            1700000001
        )
        .await
        .status_code(),
        503
    );
    {
        let logs = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
        assert!(logs.contains("Stripe webhook failed"), "{logs}");
        assert!(logs.contains("checkout.session.completed"), "{logs}");
    }
    f.calls.lock().unwrap().stripe.fail = false;
    for _ in 0..2 {
        assert_eq!(
            f.event(
                "evt_retry_paid",
                "checkout.session.completed",
                object.clone(),
                1700000001
            )
            .await
            .status_code(),
            200
        );
    }
    assert!(store(&f, &signed).subscription_active().await.unwrap());
}

#[tokio::test]
#[serial]
async fn checkout_charges_the_catalog_price_and_stripe_sells_it_through_managed_payments() {
    let f = Fixture::new(false).await;
    let session = f.subscribe().await;
    assert_eq!(session["amount_subtotal"], 4900);
    assert_eq!(session["amount_total"], 5831);
    let signed = signed(&f).await;
    assert!(store(&f, &signed).subscription_active().await.unwrap());
    let calls = f.calls.lock().unwrap();
    let checkout = calls.stripe.posted("checkout/sessions")[0];
    for (name, value) in [
        ("line_items[0][price]", "price_fixture"),
        ("line_items[0][quantity]", "1"),
        ("managed_payments[enabled]", "true"),
    ] {
        assert_eq!(
            checkout.get(name).map(String::as_str),
            Some(value),
            "{name}"
        );
    }
    for refused_by_managed_payments in [
        "automatic_tax[enabled]",
        "tax_id_collection[enabled]",
        "customer_update[address]",
        "customer_update[name]",
    ] {
        assert!(
            !checkout.contains_key(refused_by_managed_payments),
            "{refused_by_managed_payments}"
        );
    }
}

async fn stripe_setup(f: &Fixture, host: &str) -> loco_rs::Result<()> {
    let mut ctx = f.ctx.clone();
    ctx.config.server.host = host.into();
    loco_rs::boot::run_task::<App>(
        &ctx,
        Some(&"stripe_setup".to_owned()),
        &loco_rs::task::Vars::default(),
    )
    .await
}

#[tokio::test]
#[serial]
async fn the_operator_prepares_stripe_and_a_second_run_creates_nothing_more() {
    let f = Fixture::new(false).await;
    f.calls.lock().unwrap().stripe.unprepared = true;
    let response = f
        .request
        .post("/subscription")
        .add_header("origin", ORIGIN)
        .add_cookie(f.a.clone())
        .await;
    assert_eq!(response.status_code(), 503);
    assert_eq!(response.text(), "Stripe is unavailable. Try again later.");
    assert!(f
        .calls
        .lock()
        .unwrap()
        .stripe
        .posted("checkout/sessions")
        .is_empty());

    for _ in 0..2 {
        stripe_setup(&f, "https://keysend.example").await.unwrap();
    }
    {
        let calls = f.calls.lock().unwrap();
        let tax = calls.stripe.posted("tax/settings");
        assert_eq!(tax.len(), 2);
        assert_eq!(tax[0]["defaults[tax_code]"], "txcd_10103001");
        assert_eq!(tax[0]["defaults[tax_behavior]"], "exclusive");
        let products = calls.stripe.posted("products");
        assert_eq!(products.len(), 1);
        assert_eq!(products[0]["id"], "keysend_subscription");
        assert_eq!(products[0]["tax_code"], "txcd_10103001");
        let prices = calls.stripe.posted("prices");
        assert_eq!(prices.len(), 1);
        for (name, value) in [
            ("product", "keysend_subscription"),
            ("lookup_key", "keysend_monthly"),
            ("currency", "usd"),
            ("unit_amount", "4900"),
            ("recurring[interval]", "month"),
            ("tax_behavior", "exclusive"),
        ] {
            assert_eq!(prices[0][name], value, "{name}");
        }
        let portal = calls.stripe.posted("billing_portal/configurations");
        assert_eq!(portal.len(), 1);
        assert_eq!(portal[0]["features[invoice_history][enabled]"], "true");
        assert_eq!(
            portal[0]["features[subscription_cancel][mode]"],
            "at_period_end"
        );
        assert_eq!(
            calls
                .stripe
                .posted("billing_portal/configurations/bpc_fixture")
                .len(),
            1
        );
        let webhooks = calls.stripe.posted("webhook_endpoints");
        assert_eq!(webhooks.len(), 1);
        assert_eq!(webhooks[0]["url"], "https://keysend.example/stripe/webhook");
        assert_eq!(webhooks[0]["api_version"], "2025-03-31.basil");
        let mut events = (0..5)
            .map(|index| webhooks[0][&format!("enabled_events[{index}]")].as_str())
            .collect::<Vec<_>>();
        events.sort_unstable();
        assert_eq!(
            events,
            [
                "checkout.session.async_payment_succeeded",
                "checkout.session.completed",
                "customer.subscription.created",
                "customer.subscription.deleted",
                "customer.subscription.updated",
            ]
        );
        assert_eq!(calls.stripe.posted("webhook_endpoints/we_fixture").len(), 1);
    }

    f.subscribe().await;
    let signed = signed(&f).await;
    assert!(store(&f, &signed).subscription_active().await.unwrap());
}

#[tokio::test]
#[serial]
async fn stripe_setup_stops_while_stripe_tax_is_not_active_and_makes_no_webhook_for_localhost() {
    let f = Fixture::new(false).await;
    {
        let mut calls = f.calls.lock().unwrap();
        calls.stripe.unprepared = true;
        calls.stripe.tax_pending = true;
    }
    let error = stripe_setup(&f, "http://localhost").await.unwrap_err();
    assert!(error.to_string().contains("head office address"), "{error}");
    assert!(f.calls.lock().unwrap().stripe.posted("products").is_empty());

    f.calls.lock().unwrap().stripe.tax_pending = false;
    stripe_setup(&f, "http://localhost").await.unwrap();
    let calls = f.calls.lock().unwrap();
    assert_eq!(calls.stripe.posted("prices").len(), 1);
    assert!(calls.stripe.posted("webhook_endpoints").is_empty());
}

#[tokio::test]
#[serial]
async fn a_failed_payment_asks_for_another_card_and_starts_no_second_subscription() {
    let f = Fixture::new(false).await;
    f.subscribe().await;
    let past_due = {
        let mut calls = f.calls.lock().unwrap();
        let value = calls.stripe.subscriptions.get_mut("sub_fixture").unwrap();
        value["status"] = json!("past_due");
        value.clone()
    };
    assert_eq!(
        f.event(
            "evt_past_due",
            "customer.subscription.updated",
            past_due,
            1800000000
        )
        .await
        .status_code(),
        200
    );
    let page = f.page("/subscription").await;
    assert!(page.contains("The last payment did not go through."));
    assert!(page.contains("Change the payment card"));
    assert!(!page.contains("Start the Subscription"));
    let response = f
        .request
        .post("/subscription")
        .add_header("origin", ORIGIN)
        .add_cookie(f.a.clone())
        .await;
    assert_eq!(response.status_code(), 303);
    assert_eq!(response.header("location"), "/subscription");
    assert_eq!(
        f.calls
            .lock()
            .unwrap()
            .stripe
            .posted("checkout/sessions")
            .len(),
        1
    );
}

#[tokio::test]
#[serial]
async fn a_subscription_that_schedules_cancellation_and_ends_records_each_change() {
    let f = Fixture::new(false).await;
    f.subscribe().await;
    for (id, status, scheduled) in [
        ("evt_scheduled", "active", true),
        ("evt_ended", "canceled", false),
    ] {
        let object = {
            let mut calls = f.calls.lock().unwrap();
            calls
                .stripe
                .cancellation("sub_fixture", scheduled, Some(1700000100));
            let value = calls.stripe.subscriptions.get_mut("sub_fixture").unwrap();
            value["status"] = json!(status);
            value.clone()
        };
        assert_eq!(
            f.event(
                id,
                "customer.subscription.updated",
                object,
                chrono::Utc::now().timestamp()
            )
            .await
            .status_code(),
            200
        );
    }
    let signed = identity::authenticate(&f.ctx.db, f.a.value())
        .await
        .unwrap()
        .unwrap();
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let rows = store.subscription_changes().await.unwrap();
    let changes: Vec<_> = rows
        .iter()
        .map(|row| (row.status.as_str(), row.cancel_at_period_end))
        .collect();
    assert_eq!(
        changes,
        [("active", false), ("active", true), ("canceled", false)]
    );
    let subscription = store.subscription().await.unwrap().unwrap();
    assert_eq!(subscription.canceled_at, Some(1700000100));
    assert!(!store.subscription_active().await.unwrap());
}
