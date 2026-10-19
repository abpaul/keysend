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

fn discovered_campaign(id: i64, name: &str, status: &str) -> Value {
    json!({"campaign":{"id":id.to_string(),"name":name,"status":status,"advertisingChannelType":"SEARCH","biddingStrategyType":"MAXIMIZE_CONVERSIONS"},"campaignBudget":{"amountMicros":"80000000"}})
}

async fn with_new_campaign() -> Fixture {
    let f = Fixture::monitoring().await;
    collect(&f).await.unwrap();
    f.calls.lock().unwrap().google.monitoring_campaigns = Some(vec![
        discovered_campaign(701, "Search - Real", "ENABLED"),
        discovered_campaign(702, "Search - Brand New", "ENABLED"),
    ]);
    collect(&f).await.unwrap();
    f
}

#[tokio::test]
#[serial]
async fn a_user_sees_a_new_enabled_campaign_without_activity_once_and_opens_its_page() {
    let f = with_new_campaign().await;
    for _ in 0..2 {
        let page = f.page("/ad-accounts/4829153307").await;
        assert_eq!(
            page.matches("Search - Brand New").count(),
            1,
            "the Ad Account page lists the new Campaign once"
        );
        assert!(page.contains(r#"href="/ad-accounts/4829153307/campaigns/702""#));
        collect(&f).await.unwrap();
    }
    let page = f.page("/ad-accounts/4829153307/campaigns/702").await;
    assert!(page.contains("Search - Brand New"));
    assert!(!page.contains("NaN"));
    assert!(page.contains("EUR 0"));
}

#[tokio::test]
#[serial]
async fn a_user_exports_a_new_enabled_campaign_without_activity() {
    let f = with_new_campaign().await;
    let signed = identity::authenticate(&f.ctx.db, f.a.value())
        .await
        .unwrap()
        .unwrap();
    let words = export_words(&f, &signed, "4829153307", None, "7").await;
    assert!(
        words.contains("Search - Brand New"),
        "the Export lists the new Campaign"
    );
    assert!(!words.contains("NaN"));
}

#[tokio::test]
#[serial]
async fn a_user_keeps_campaign_history_through_renames_status_changes_and_collection_failures() {
    let f = Fixture::monitoring().await;
    collect(&f).await.unwrap();
    let signed = identity::authenticate(&f.ctx.db, f.a.value())
        .await
        .unwrap()
        .unwrap();
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    f.calls.lock().unwrap().google.monitoring_campaigns = Some(vec![
        discovered_campaign(701, "Search - Renamed", "PAUSED"),
        discovered_campaign(703, "Search - Retired", "REMOVED"),
        discovered_campaign(704, "Search - Waiting", "PAUSED"),
    ]);
    // An old day remains accessible on the Campaign page, outside both Windows.
    store
        .save_history(
            "4829153307",
            NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(),
            NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(),
            &[],
            &[keysend::workspace::history::CampaignDay {
                day: NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(),
                campaign_id: 703,
                device: 1,
                counts: keysend::monitoring::audit::Counts {
                    impressions: 100.0,
                    clicks: 10.0,
                    cost: 10.0,
                    ..Default::default()
                },
            }],
            &[],
        )
        .await
        .unwrap();
    collect(&f).await.unwrap();
    for window in ["7", "30"] {
        let page = f
            .page(&format!("/ad-accounts/4829153307?window={window}"))
            .await;
        assert!(page.contains("Search - Renamed"));
        assert!(!page.contains("Search - Real"));
        assert!(!page.contains("Search - Retired"));
        assert!(!page.contains("Search - Waiting"));
        let words = export_words(&f, &signed, "4829153307", None, window).await;
        assert!(words.contains("Search - Renamed"));
        assert!(!words.contains("Search - Retired"));
        assert!(!words.contains("Search - Waiting"));
    }
    let page = f
        .page("/ad-accounts/4829153307/campaigns/701?window=30")
        .await;
    assert!(page.contains("Search - Renamed"));
    assert!(
        page.contains("EUR 300"),
        "the renamed Campaign keeps its daily spend"
    );
    assert!(f
        .page("/ad-accounts/4829153307/campaigns/703")
        .await
        .contains("Search - Retired"));
    // The current Window cannot expose the old day's counts.
    assert_eq!(
        store
            .window_metrics(
                "4829153307",
                keysend::workspace::history::day_number(
                    NaiveDate::from_ymd_opt(2025, 1, 1).unwrap()
                ),
                keysend::monitoring::metrics::Window::SevenDays,
                Some(703)
            )
            .await
            .unwrap()
            .current
            .spend,
        10.0
    );
    f.calls.lock().unwrap().google.failed_status =
        Some(axum::http::StatusCode::SERVICE_UNAVAILABLE);
    assert!(collect(&f).await.is_err());
    assert!(f
        .page("/ad-accounts/4829153307")
        .await
        .contains("Search - Renamed"));
    assert!(store
        .history_state("4829153307")
        .await
        .unwrap()
        .unwrap()
        .error
        .is_some());
    f.calls.lock().unwrap().google.failed_status = None;
    collect(&f).await.unwrap();
    assert_eq!(
        store
            .history_state("4829153307")
            .await
            .unwrap()
            .unwrap()
            .error,
        None
    );
}

#[tokio::test]
#[serial]
async fn a_user_sees_only_the_campaign_names_of_their_workspace_after_collection() {
    let f = Fixture::monitoring().await;
    collect(&f).await.unwrap();
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
    f.calls.lock().unwrap().google.monitoring_campaigns = Some(vec![discovered_campaign(
        701,
        "Search - Workspace B",
        "ENABLED",
    )]);
    google_ads::collect_history(&Settings::load(&f.ctx).unwrap(), &store, "4829153307")
        .await
        .unwrap();
    let first = f.page("/ad-accounts/4829153307").await;
    assert!(first.contains("Search - Real"));
    assert!(!first.contains("Search - Workspace B"));
    let second = f
        .request
        .get("/ad-accounts/4829153307")
        .add_cookie(f.b.clone())
        .await;
    assert_eq!(second.status_code(), 200);
    assert!(second.text().contains("Search - Workspace B"));
    assert!(!second.text().contains("Search - Real"));
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
#[ignore = "serves newly discovered Campaigns for a browser check"]
async fn a_user_views_a_new_campaign_in_a_browser() {
    let directory = std::path::PathBuf::from(std::env::var("KEYSEND_BROWSER_CHECK_DIR").unwrap());
    let f = with_new_campaign().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    std::fs::write(
        directory.join("address"),
        listener.local_addr().unwrap().to_string(),
    )
    .unwrap();
    std::fs::write(directory.join("cookie"), f.a.value()).unwrap();
    let router = f.router.clone();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    while !directory.join("stop").exists() {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

#[tokio::test]
#[serial]
async fn an_operator_reads_the_number_of_new_campaigns_stored_for_each_ad_account() {
    let (captured, _subscriber) = capture();
    let f = with_new_campaign().await;
    collect(&f).await.unwrap();
    let logs = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
    let discoveries = logs
        .lines()
        .filter(|line| line.contains("Campaign discovery stored"))
        .collect::<Vec<_>>();
    assert_eq!(discoveries.len(), 3, "{logs}");
    for (line, count) in discoveries.iter().zip([1, 1, 0]) {
        assert!(line.contains("INFO"), "{line}");
        assert!(line.contains("ad_account=\"4829153307\""), "{line}");
        assert!(line.contains(&format!("new_campaigns={count}")), "{line}");
        assert!(!line.contains("Search -"), "{line}");
    }
}
