use super::*;
use chrono::{Days, NaiveDate};
use keysend::{google_ads, settings::Settings};

async fn collect(f: &Fixture) -> Result<(), String> {
    let signed = identity::authenticate(&f.ctx.db, f.a.value())
        .await
        .unwrap()
        .unwrap();
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    google_ads::collect_history(&Settings::load(&f.ctx).unwrap(), &store, "4829153307")
        .await
        .map_err(|error| error.to_string())
}

#[tokio::test]
#[serial]
async fn revoked_disabled_and_rate_limited_google_access_stop_cleanly() {
    for (kind, expected) in [
        ("revoked", "Connect Google Ads again."),
        ("disabled", "This Ad Account is not enabled in Google Ads."),
        (
            "quota",
            "Google Ads quota or rate limit reached. Try again later.",
        ),
    ] {
        let f = Fixture::configured_source(false, false, true, "google").await;
        {
            let mut calls = f.calls.lock().unwrap();
            calls.google.revoked = kind == "revoked";
            calls.google.disabled = kind == "disabled";
            if kind == "quota" {
                calls.google.failed_status = Some(axum::http::StatusCode::TOO_MANY_REQUESTS);
            }
        }
        assert_eq!(collect(&f).await, Err(expected.to_owned()), "{kind}");
        let signed = identity::authenticate(&f.ctx.db, f.a.value())
            .await
            .unwrap()
            .unwrap();
        let state = WorkspaceStore::new(&f.ctx.db, &signed.workspace)
            .history_state("4829153307")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(state.error.as_deref(), Some(expected), "{kind}");
        assert!(state.last_day.is_none());
        assert!(f.calls.lock().unwrap().commentary.is_empty());
    }
}

#[tokio::test]
#[serial]
async fn the_google_operation_limit_stops_requests_and_does_not_reset_with_a_workspace() {
    let f =
        Fixture::configured_clock(false, false, true, "google", 1, "2026-10-11T12:00:00Z").await;
    assert_eq!(
        collect(&f).await,
        Err("Google Ads quota or rate limit reached. Try again later.".to_owned())
    );
    assert_eq!(f.calls.lock().unwrap().google.requests.len(), 1);
    let signed = identity::authenticate(&f.ctx.db, f.b.value())
        .await
        .unwrap()
        .unwrap();
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let connection = store
        .save_google_connection(
            "b",
            "b@example.com",
            "never-send-this-refresh-secret",
            &MasterKey::from_base64(KEY).unwrap(),
        )
        .await
        .unwrap();
    let selection = store
        .create_selection(
            &connection.id,
            vec![AdAccountChoice {
                customer_id: "4829153307".into(),
                name: "Workspace B Ad Account".into(),
                login_customer_id: None,
            }],
        )
        .await
        .unwrap();
    assert!(store
        .save_selected(&selection, &["4829153307".into()])
        .await
        .unwrap());
    assert_eq!(
        google_ads::collect_history(&Settings::load(&f.ctx).unwrap(), &store, "4829153307")
            .await
            .unwrap_err()
            .to_string(),
        "Google Ads quota or rate limit reached. Try again later."
    );
    assert_eq!(f.calls.lock().unwrap().google.requests.len(), 1);
}

#[tokio::test]
#[serial]
async fn google_calls_never_put_credentials_in_logs() {
    let (captured, _subscriber) = capture();
    let f = Fixture::configured_source(false, false, true, "google").await;
    f.calls.lock().unwrap().google.revoked = true;
    assert!(collect(&f).await.is_err());
    let logs = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("Google call failed"));
    for secret in [
        "google-access-secret",
        "google-client-secret",
        "never-send-this-refresh-secret",
        "stub-openai-secret",
        "sk_test_stub-stripe-secret",
        "whsec_stub-stripe-secret",
        f.a.value(),
        KEY,
    ] {
        assert!(!logs.contains(secret), "a secret reached the log");
    }
}

#[tokio::test]
#[serial]
async fn google_query_failures_log_only_known_codes_and_resource() {
    let (captured, _subscriber) = capture();
    let f = Fixture::configured_source(false, false, true, "google").await;
    {
        let mut calls = f.calls.lock().unwrap();
        calls.google.failed_status = Some(axum::http::StatusCode::BAD_REQUEST);
        calls.google.failed_body = Some(
            json!({"error":{"message":"google-access-secret never-send-this-refresh-secret", "details":[{"errors":[{"errorCode":{"queryError":"PROHIBITED_METRIC_IN_SELECT_OR_WHERE_CLAUSE"}},{"errorCode":{"queryError":"stub-openai-secret"}}]}]}}),
        );
    }
    assert_eq!(
        collect(&f).await,
        Err("Google Ads data is unavailable. Try again later.".to_owned())
    );
    let logs = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("Google Metrics request failed"));
    assert!(logs.contains("resource=\"customer\""));
    assert!(logs.contains("http_status=400"));
    assert!(logs.contains("query_kind=\"metadata\""));
    assert!(logs.contains("PROHIBITED_METRIC_IN_SELECT_OR_WHERE_CLAUSE"));
    assert!(logs.contains("OTHER"));
    for secret in [
        "google-access-secret",
        "never-send-this-refresh-secret",
        "stub-openai-secret",
        KEY,
        f.a.value(),
    ] {
        assert!(!logs.contains(secret), "a secret reached the log");
    }
}

#[tokio::test]
#[serial]
async fn account_history_is_collected_once_then_only_the_last_days_again() {
    let f = Fixture::monitoring().await;
    let signed = identity::authenticate(&f.ctx.db, f.a.value())
        .await
        .unwrap()
        .unwrap();
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let day_queries = |f: &Fixture| {
        f.calls
            .lock()
            .unwrap()
            .google
            .requests
            .iter()
            .filter(|(_, body)| {
                body["query"]
                    .as_str()
                    .unwrap()
                    .contains("segments.date, segments.device")
            })
            .count()
    };
    collect(&f).await.unwrap();
    assert_eq!(day_queries(&f), 9);
    let state = store.history_state("4829153307").await.unwrap().unwrap();
    assert_eq!(state.last_day.unwrap().to_string(), "2026-10-11");
    assert_eq!(state.first_day.unwrap().to_string(), "2024-10-12");
    assert_eq!(state.error, None);
    let campaigns = store.campaigns("4829153307").await.unwrap();
    assert_eq!(campaigns.len(), 1);
    assert_eq!(campaigns[0].name, "Search - Real");
    collect(&f).await.unwrap();
    assert_eq!(day_queries(&f), 10);
    let other = identity::authenticate(&f.ctx.db, f.b.value())
        .await
        .unwrap()
        .unwrap();
    let other = WorkspaceStore::new(&f.ctx.db, &other.workspace);
    assert!(other.history_state("4829153307").await.unwrap().is_none());
    assert!(other.campaigns("4829153307").await.unwrap().is_empty());
}

fn operations(f: &Fixture) -> Vec<String> {
    f.calls
        .lock()
        .unwrap()
        .google
        .requests
        .iter()
        .map(|(_, body)| body["query"].as_str().unwrap().to_owned())
        .collect()
}

fn from_day(query: &str) -> &str {
    &query.split("segments.date BETWEEN '").nth(1).unwrap()[..10]
}

fn of_kind<'a>(queries: &'a [String], part: &str) -> Vec<&'a String> {
    queries
        .iter()
        .filter(|query| query.contains(part))
        .collect()
}

#[tokio::test]
#[serial]
async fn a_new_ad_account_reads_the_breakdowns_only_for_the_days_the_pages_show() {
    let f = Fixture::monitoring().await;
    collect(&f).await.unwrap();
    let queries = operations(&f);
    let end = NaiveDate::from_ymd_opt(2026, 10, 11).unwrap();
    for part in [
        "metrics.search_impression_share",
        "segments.conversion_action_name",
        "segments.ad_network_type",
    ] {
        let found = of_kind(&queries, part);
        let mut from = found
            .iter()
            .map(|query| from_day(query))
            .collect::<Vec<_>>();
        from.sort();
        assert_eq!(
            from,
            [
                end.checked_sub_days(Days::new(83)).unwrap().to_string(),
                end.checked_sub_days(Days::new(59)).unwrap().to_string()
            ],
            "{part}"
        );
    }
    assert_eq!(of_kind(&queries, "segments.date, segments.device").len(), 9);
    assert_eq!(queries.len(), 18);
    let before = queries.len();
    collect(&f).await.unwrap();
    assert_eq!(operations(&f).len() - before, 6);
}

#[tokio::test]
#[serial]
async fn the_daily_history_of_every_ad_account_comes_before_the_backfill_of_another() {
    let (captured, _subscriber) = capture();
    let f = Fixture::monitoring_limit(14).await;
    let signed = identity::authenticate(&f.ctx.db, f.b.value())
        .await
        .unwrap()
        .unwrap();
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let connection = store
        .save_google_connection(
            "b",
            "b@example.com",
            "never-send-this-refresh-secret",
            &MasterKey::from_base64(KEY).unwrap(),
        )
        .await
        .unwrap();
    let selection = store
        .create_selection(
            &connection.id,
            vec![AdAccountChoice {
                customer_id: "4829153307".into(),
                name: "Workspace B Ad Account".into(),
                login_customer_id: None,
            }],
        )
        .await
        .unwrap();
    assert!(store
        .save_selected(&selection, &["4829153307".into()])
        .await
        .unwrap());

    collect(&f).await.unwrap();
    assert_eq!(operations(&f).len(), 8);
    let first = identity::authenticate(&f.ctx.db, f.a.value())
        .await
        .unwrap()
        .unwrap();
    let state = WorkspaceStore::new(&f.ctx.db, &first.workspace)
        .history_state("4829153307")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.error, None);
    assert_eq!(state.last_day.unwrap().to_string(), "2026-10-11");

    google_ads::collect_history(&Settings::load(&f.ctx).unwrap(), &store, "4829153307")
        .await
        .unwrap();
    assert_eq!(operations(&f).len(), 12);
    let state = store.history_state("4829153307").await.unwrap().unwrap();
    assert_eq!(state.error, None);
    assert_eq!(state.last_day.unwrap().to_string(), "2026-10-11");
    let logs = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("daily Account history"), "{logs}");
}

#[tokio::test]
#[serial]
async fn the_daily_limit_fails_the_daily_history_with_a_logged_reason() {
    let (captured, _subscriber) = capture();
    let f = Fixture::monitoring_limit(2).await;
    assert_eq!(
        collect(&f).await,
        Err("Google Ads quota or rate limit reached. Try again later.".to_owned())
    );
    let logs = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
    assert!(
        logs.contains("Google Ads daily operation limit reached; no request was sent"),
        "{logs}"
    );
    assert!(logs.contains("ERROR"), "{logs}");
}
