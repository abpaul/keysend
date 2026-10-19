use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;

enum Reply {
    Close,
    Respond(u16, &'static str),
}

fn complete(request: &[u8]) -> bool {
    let text = String::from_utf8_lossy(request);
    let Some((head, body)) = text.split_once("\r\n\r\n") else {
        return false;
    };
    let length = head
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .and_then(|value| value.trim().parse::<usize>().ok())
        })
        .unwrap_or(0);
    body.len() >= length
}

async fn email_service(reply: Reply) -> (String, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/email", listener.local_addr().unwrap());
    let requests = Arc::new(AtomicUsize::new(0));
    let counted = requests.clone();
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 4096];
            while !complete(&request) {
                let read = socket.read(&mut buffer).await.unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
            }
            counted.fetch_add(1, Ordering::SeqCst);
            if let Reply::Respond(status, body) = &reply {
                let response = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
                socket.shutdown().await.ok();
            }
        }
    });
    (url, requests)
}

async fn send(f: &Fixture, job: &MonitoringJob) {
    worker::send(
        &f.ctx,
        &EmailJob {
            workspace_id: job.workspace_id.clone(),
        },
    )
    .await
    .unwrap();
}

fn use_email_service(f: &mut Fixture, url: &str) {
    f.ctx.config.settings.as_mut().unwrap()["services"]["email_url"] = json!(url);
}

async fn closed_port_url() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    format!("http://{}/email", listener.local_addr().unwrap())
}

fn restrict_recipient(f: &Fixture, recipient: Option<&str>) {
    match recipient {
        Some(recipient) => env::set_var("REPORT_EMAIL_RECIPIENT", recipient),
        None => env::remove_var("REPORT_EMAIL_RECIPIENT"),
    }
    let credentials =
        keysend::settings::Credentials::resolve(&f.ctx.environment, &f.ctx.config).unwrap();
    f.ctx.shared_store.insert(Arc::new(credentials));
}

fn assert_weekly_email(f: &Fixture) {
    let calls = f.calls.lock().unwrap();
    assert_eq!(calls.emails.len(), 1);
    let email = &calls.emails[0];
    assert_eq!(email["from"], "hello@keysend.app");
    assert_eq!(email["to"], json!(["a@example.com"]));
    assert_eq!(
        email["subject"],
        "Your week in Google Ads: Northwind Dental"
    );
    assert!(email["text"]
        .as_str()
        .unwrap()
        .contains("Spend: EUR 70.00 (no change)\nConversions: 7 (no change)"));
    assert!(email["html"]
        .as_str()
        .unwrap()
        .contains("Open Northwind Dental"));
}

#[tokio::test]
#[serial]
async fn a_user_receives_a_weekly_email_without_a_recipient_restriction() {
    let f = Fixture::monitoring().await;
    restrict_recipient(&f, None);
    let job = f.prepare_monitoring().await;
    worker::run(&f.ctx, &job).await.unwrap();
    assert_weekly_email(&f);
    send(&f, &job).await;
    assert_weekly_email(&f);
}

#[tokio::test]
#[serial]
async fn a_user_receives_a_weekly_email_with_an_empty_recipient_restriction() {
    let f = Fixture::monitoring().await;
    restrict_recipient(&f, Some(""));
    let job = f.prepare_monitoring().await;
    worker::run(&f.ctx, &job).await.unwrap();
    assert_weekly_email(&f);
}

#[tokio::test]
#[serial]
async fn a_user_receives_a_weekly_email_when_the_recipient_restriction_matches() {
    let f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    worker::run(&f.ctx, &job).await.unwrap();
    assert_weekly_email(&f);
}

#[tokio::test]
#[serial]
async fn a_user_outside_the_recipient_restriction_receives_no_weekly_email() {
    let f = Fixture::monitoring().await;
    restrict_recipient(&f, Some("other@example.com"));
    let job = f.prepare_monitoring().await;
    worker::run(&f.ctx, &job).await.unwrap();
    assert_eq!(f.calls.lock().unwrap().emails.len(), 0);

    restrict_recipient(&f, Some("a@example.com"));
    send(&f, &job).await;
    assert_weekly_email(&f);
}

#[tokio::test]
#[serial]
async fn a_user_receives_an_alert_without_a_recipient_restriction() {
    let f = Fixture::monitoring().await;
    restrict_recipient(&f, None);
    let job = f.prepare_monitoring().await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = Settings::load(&f.ctx).unwrap().now().timestamp();
    let alert = keysend::monitoring::alerts::Alert {
        day: day_number(end()),
        event_key: keysend::monitoring::alerts::EventKey::SpendDoubled,
        campaign_id: None,
        name: "Northwind Dental".into(),
        metrics: keysend::monitoring::metrics::Metrics {
            spend: 20.0,
            ..Default::default()
        },
        normal_value: 10.0,
        body: "Northwind Dental: spend doubled.".into(),
    };
    store.save_alerts(AD, &[alert], now - 3600).await.unwrap();

    send(&f, &job).await;
    send(&f, &job).await;
    let calls = f.calls.lock().unwrap();
    assert_eq!(calls.emails.len(), 1);
    let email = &calls.emails[0];
    assert_eq!(email["to"], json!(["a@example.com"]));
    assert_eq!(email["subject"], "Alert: Spend doubled in Northwind Dental");
    assert!(email["text"].as_str().unwrap().contains(
        "Spend on Sunday 11 October was EUR 20.00, at least twice the normal EUR 10.00 for a Sunday."
    ));
    assert!(email["html"]
        .as_str()
        .unwrap()
        .contains("Open Northwind Dental"));
}

#[tokio::test]
#[serial]
async fn email_delivery_sends_only_to_users_allowed_by_the_optional_restriction() {
    let f = Fixture::monitoring().await;
    for (restriction, allowed) in [
        (None, true),
        (Some("a@example.com"), true),
        (Some("other@example.com"), false),
    ] {
        let mut settings = Settings::load(&f.ctx).unwrap();
        settings.email_recipient = restriction.map(str::to_owned);
        let result = keysend::monitoring::email::deliver(
            &settings,
            "a@example.com",
            "Your Google Ads Alert",
            "Harbour Dental: spend doubled.",
            Some("<p>Harbour Dental: spend doubled.</p>"),
        )
        .await;
        assert_eq!(result.is_ok(), allowed, "restriction {restriction:?}");
    }
    assert_eq!(
        f.calls.lock().unwrap().emails,
        vec![
            json!({
                "from": "hello@keysend.app", "to": ["a@example.com"],
                "subject": "Your Google Ads Alert", "text": "Harbour Dental: spend doubled.",
                "html": "<p>Harbour Dental: spend doubled.</p>"
            }),
            json!({
                "from": "hello@keysend.app", "to": ["a@example.com"],
                "subject": "Your Google Ads Alert", "text": "Harbour Dental: spend doubled.",
                "html": "<p>Harbour Dental: spend doubled.</p>"
            }),
        ]
    );
}

#[tokio::test]
#[serial]
async fn a_weekly_email_whose_outcome_is_unknown_is_not_sent_again() {
    let (captured, _subscriber) = capture();
    let mut f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    let (url, requests) = email_service(Reply::Close).await;
    use_email_service(&mut f, &url);
    worker::run(&f.ctx, &job).await.unwrap();
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    let logs = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
    assert!(
        logs.contains("ERROR")
            && logs.contains("The outcome of the email is unknown; it is not sent again")
            && logs.contains(r#"kind="other""#),
        "{logs}"
    );

    send(&f, &job).await;
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    f.ctx.config.settings.as_mut().unwrap()["services"]["test_now"] = json!("2026-10-12T12:11:00Z");
    send(&f, &job).await;
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    assert!(f.calls.lock().unwrap().emails.is_empty());
    let logs = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
    assert!(
        logs.contains("ERROR")
            && logs.contains("Email sending was not confirmed; it is not sent again")
            && logs.contains(r#"kind="weekly""#)
            && logs.contains(r#"ad_accounts=["4829153307"]"#),
        "{logs}"
    );
}

#[tokio::test]
#[serial]
async fn a_weekly_email_that_the_service_did_not_accept_is_sent_again() {
    let mut f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    let (url, requests) = email_service(Reply::Respond(503, "{}")).await;
    use_email_service(&mut f, &url);
    assert!(worker::run(&f.ctx, &job).await.is_err());
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    let closed = closed_port_url().await;
    use_email_service(&mut f, &closed);
    assert!(worker::run(&f.ctx, &job).await.is_err());
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    assert_eq!(store.pending_weekly_emails().await.unwrap().len(), 1);
}

#[tokio::test]
#[serial]
async fn a_success_status_is_a_sent_email_even_when_the_body_cannot_be_read() {
    let mut f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    let (url, requests) = email_service(Reply::Respond(202, "not json")).await;
    use_email_service(&mut f, &url);
    worker::run(&f.ctx, &job).await.unwrap();
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    assert!(store.pending_weekly_emails().await.unwrap().is_empty());
}

#[tokio::test]
#[serial]
async fn an_alert_whose_outcome_is_unknown_is_not_sent_again() {
    let mut f = Fixture::monitoring().await;
    let job = f.prepare_monitoring().await;
    let signed = signed(&f).await;
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let now = Settings::load(&f.ctx).unwrap().now().timestamp();
    let alert = keysend::monitoring::alerts::Alert {
        day: day_number(end()),
        event_key: keysend::monitoring::alerts::EventKey::SpendDoubled,
        campaign_id: None,
        name: "Harbour Dental".into(),
        metrics: Default::default(),
        normal_value: 10.0,
        body: "Harbour Dental: spend doubled.".into(),
    };
    store
        .save_alerts(AD, std::slice::from_ref(&alert), now - 3600)
        .await
        .unwrap();
    let (url, requests) = email_service(Reply::Close).await;
    use_email_service(&mut f, &url);
    send(&f, &job).await;
    send(&f, &job).await;
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    assert!(f.calls.lock().unwrap().emails.is_empty());
}
