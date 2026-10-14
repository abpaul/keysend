use std::{
    collections::HashMap,
    env,
    sync::{Arc, Mutex},
};

use axum::{
    extract::{Form, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use axum_extra::extract::cookie::{Cookie, SameSite};
use axum_test::{TestResponse, TestServer};
use keysend::{
    app::App,
    auth::{OAUTH_COOKIE, SESSION_COOKIE},
    crypto::MasterKey,
    identity,
    settings::Settings,
};
use loco_rs::{
    app::{AppContext, Hooks},
    boot::StartMode,
    environment::Environment,
};
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use serde_json::{json, Value};
use serial_test::serial;
use tokio::task::JoinHandle;

const KEY: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
const ORIGIN: &str = "http://localhost:5150";

#[derive(Default)]
struct Calls {
    tokens: Vec<HashMap<String, String>>,
    ads: Vec<(String, Option<String>, Value)>,
    search_failure: Option<(String, StatusCode)>,
    manager_search_failure: Option<StatusCode>,
    accessible_failure: Option<StatusCode>,
    malformed_resources: bool,
    empty_names: bool,
    refresh_revoked: bool,
}

struct EnvironmentGuard(Vec<(&'static str, Option<String>)>);
impl EnvironmentGuard {
    fn new(credentials: bool, key: bool) -> Self {
        let mut previous = Vec::new();
        for (name, value) in [
            ("GOOGLE_CLIENT_ID", credentials.then_some("stub-id")),
            ("GOOGLE_CLIENT_SECRET", credentials.then_some("stub-secret")),
            ("KEYSEND_MASTER_KEY", key.then_some(KEY)),
        ] {
            previous.push((name, env::var(name).ok()));
            match value {
                Some(value) => env::set_var(name, value),
                None => env::remove_var(name),
            }
        }
        Self(previous)
    }
}
impl Drop for EnvironmentGuard {
    fn drop(&mut self) {
        for (name, value) in &self.0 {
            match value {
                Some(value) => env::set_var(name, value),
                None => env::remove_var(name),
            }
        }
    }
}

struct Fixture {
    request: TestServer,
    ctx: AppContext,
    calls: Arc<Mutex<Calls>>,
    stub_url: String,
    stub: JoinHandle<()>,
    _environment: EnvironmentGuard,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stub.abort();
    }
}

impl Fixture {
    async fn new() -> Self {
        Self::configured(true, true, false).await
    }

    async fn configured(credentials: bool, key: bool, https: bool) -> Self {
        let environment = EnvironmentGuard::new(credentials, key);
        let calls = Arc::new(Mutex::new(Calls::default()));
        let stub = Router::new()
            .route("/token", post(token))
            .route("/userinfo", get(userinfo))
            .route("/v25/customers:listAccessibleCustomers", get(accessible))
            .route("/v25/customers/{id}/googleAds:search", post(search))
            .with_state(calls.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let stub_url = format!("http://{}", listener.local_addr().unwrap());
        let stub = tokio::spawn(async move {
            axum::serve(listener, stub).await.unwrap();
        });
        let mut config = App::load_config(&Environment::Test).await.unwrap();
        config.database.uri = "sqlite::memory:".into();
        config.queue = None;
        if https {
            config.server.host = "https://keysend.example".into();
        }
        config.settings = Some(json!({"google": {
            "authorization_url": format!("{stub_url}/authorize"), "token_url": format!("{stub_url}/token"),
            "userinfo_url": format!("{stub_url}/userinfo"), "ads_api_base": format!("{stub_url}/v25"),
        }}));
        let boot = App::boot(StartMode::ServerOnly, &Environment::Test, config)
            .await
            .unwrap();
        let request = TestServer::new(boot.router.unwrap()).unwrap();
        Self {
            request,
            ctx: boot.app_context,
            calls,
            stub_url,
            stub,
            _environment: environment,
        }
    }

    async fn begin(
        &self,
        session: Option<&Cookie<'static>>,
    ) -> (String, Cookie<'static>, reqwest::Url) {
        let response = if let Some(session) = session {
            self.request
                .post("/connections/google/start")
                .add_header("origin", ORIGIN)
                .add_cookie(session.clone())
                .await
        } else {
            self.request.get("/auth/google/start").await
        };
        assert_eq!(response.status_code(), 303);
        let url = reqwest::Url::parse(response.header("location").to_str().unwrap()).unwrap();
        assert_eq!(url.origin().ascii_serialization(), self.stub_url);
        let state = url
            .query_pairs()
            .find(|(key, _)| key == "state")
            .unwrap()
            .1
            .into_owned();
        (state, response.cookie(OAUTH_COOKIE), url)
    }

    async fn callback(
        &self,
        state: &str,
        browser: &Cookie<'static>,
        code: &str,
        session: Option<&Cookie<'static>>,
    ) -> TestResponse {
        let mut request = self
            .request
            .get("/auth/google/callback")
            .add_query_param("state", state)
            .add_query_param("code", code)
            .add_cookie(browser.clone());
        if let Some(session) = session {
            request = request.add_cookie(session.clone());
        }
        request.await
    }

    async fn sign_in(&self, code: &str) -> Cookie<'static> {
        let (state, browser, _) = self.begin(None).await;
        let response = self.callback(&state, &browser, code, None).await;
        assert_eq!(response.status_code(), 303, "{}", response.text());
        response.cookie(SESSION_COOKIE)
    }

    async fn connect(&self, session: &Cookie<'static>, code: &str) -> String {
        let (state, browser, _) = self.begin(Some(session)).await;
        let response = self.callback(&state, &browser, code, Some(session)).await;
        assert_eq!(response.status_code(), 303, "{}", response.text());
        response.header("location").to_str().unwrap().into()
    }

    async fn ad_account_names(&self, session: &Cookie<'static>) -> Vec<String> {
        let signed = keysend::identity::authenticate(&self.ctx.db, session.value())
            .await
            .unwrap()
            .unwrap();
        keysend::workspace::WorkspaceStore::new(&self.ctx.db, &signed.workspace)
            .ad_accounts()
            .await
            .unwrap()
            .into_iter()
            .map(|ad| ad.name)
            .collect()
    }

    async fn count(&self, table: &str) -> i64 {
        self.ctx
            .db
            .query_one_raw(Statement::from_string(
                DatabaseBackend::Sqlite,
                format!("SELECT COUNT(*) AS count FROM {table}"),
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get("", "count")
            .unwrap()
    }
}

async fn token(
    State(calls): State<Arc<Mutex<Calls>>>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    assert!(headers.get("developer-token").is_none());
    assert_eq!(form.get("client_id").unwrap(), "stub-id");
    assert_eq!(form.get("client_secret").unwrap(), "stub-secret");
    if form.get("grant_type").unwrap() == "refresh_token" {
        let mut calls = calls.lock().unwrap();
        calls.tokens.push(form.clone());
        if calls.refresh_revoked {
            return (
                StatusCode::BAD_REQUEST,
                Json(
                    json!({"error": "invalid_grant", "error_description": format!(
                        "Token has been expired or revoked: {}", form["refresh_token"]
                    )}),
                ),
            );
        }
        assert_eq!(form.get("refresh_token").unwrap(), "secret-refresh-token");
        return (
            StatusCode::OK,
            Json(json!({"access_token": "ads-a", "token_type": "Bearer"})),
        );
    }
    assert_eq!(form.get("grant_type").unwrap(), "authorization_code");
    assert_eq!(form.get("code_verifier").unwrap().len(), 43);
    let code = form.get("code").unwrap().clone();
    calls.lock().unwrap().tokens.push(form.clone());
    if code == "denied" {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                json!({"error": "invalid_grant", "error_description": format!(
                    "Rejected authorization: {} {} {}\n{}END-OF-LONG-ERROR", code, form["code_verifier"], form["client_secret"], "x".repeat(300)
                )}),
            ),
        );
    }
    let mut value = json!({"access_token": code, "token_type": "Bearer"});
    if code.starts_with("ads-") {
        value["refresh_token"] = json!("secret-refresh-token");
    }
    (StatusCode::OK, Json(value))
}

async fn userinfo(headers: HeaderMap) -> Json<Value> {
    let code = headers
        .get("authorization")
        .unwrap()
        .to_str()
        .unwrap()
        .strip_prefix("Bearer ")
        .unwrap();
    let (subject, email, name) = match code {
        "user-b" => ("subject-b", "b@example.com", "User B"),
        "ads-b" => ("ads-subject-b", "ads-b@example.com", "Google Connection B"),
        "ads-a-2" => (
            "ads-subject-a-2",
            "ads-a-2@example.com",
            "Second Google Connection",
        ),
        "ads-a" => ("ads-subject-a", "ads-a@example.com", "Google Connection A"),
        _ => ("subject-a", "a@example.com", "User A"),
    };
    Json(
        json!({"sub": subject, "email": email, "name": name, "email_verified": code != "unverified"}),
    )
}

async fn accessible(
    State(calls): State<Arc<Mutex<Calls>>>,
    headers: HeaderMap,
) -> (StatusCode, Json<Value>) {
    assert!(headers.get("developer-token").is_none());
    assert!(headers.get("login-customer-id").is_none());
    let access = headers
        .get("authorization")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let mut calls = calls.lock().unwrap();
    calls.ads.push((access.clone(), None, Value::Null));
    if let Some(status) = calls.accessible_failure {
        return (
            status,
            Json(json!({"error": {
                "status": "PERMISSION_DENIED", "message": format!("API disabled: {access}"),
                "details": [{"errors": [{"errorCode": {"authorizationError": "PROJECT_DISABLED"}}]}]
            }})),
        );
    }
    let mut response = if access == "Bearer ads-b" {
        json!({"resourceNames": ["customers/4444444444"]})
    } else {
        json!({"resourceNames": ["customers/1111111111", "customers/3333333333"]})
    };
    let resources = response["resourceNames"].as_array_mut().unwrap();
    if let Some((id, _)) = &calls.search_failure {
        if !resources
            .iter()
            .any(|value| value == &format!("customers/{id}"))
        {
            resources.insert(0, json!(format!("customers/{id}")));
        }
    }
    if calls.malformed_resources {
        resources.insert(0, json!("customers/not-a-customer-id"));
        resources.insert(0, json!("unexpected/9999999999"));
    }
    (StatusCode::OK, Json(response))
}

async fn search(
    State(calls): State<Arc<Mutex<Calls>>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    assert!(headers.get("developer-token").is_none());
    let manager = headers
        .get("login-customer-id")
        .map(|value| value.to_str().unwrap().to_owned());
    let access = headers
        .get("authorization")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let query = body["query"].as_str().unwrap();
    if !query.contains("FROM customer_client") {
        return (StatusCode::OK, Json(json!({"results": []})));
    }
    let (failure, empty_names) = {
        let mut calls = calls.lock().unwrap();
        calls
            .ads
            .push((access.clone(), manager.clone(), body.clone()));
        let failure = if manager.is_some() {
            calls
                .manager_search_failure
                .map(|status| (id.clone(), status))
                .or_else(|| calls.search_failure.clone())
        } else {
            calls.search_failure.clone()
        };
        (failure, calls.empty_names)
    };
    if let Some((failed_id, status)) = failure {
        if id == failed_id {
            return (
                status,
                Json(json!({"error": {
                    "status": "PERMISSION_DENIED", "message": format!("Customer is unavailable: {access}"),
                    "details": [{"errors": [{"errorCode": {"authorizationError": "CUSTOMER_NOT_ENABLED"}}]}]
                }})),
            );
        }
    }
    let mut response = if id == "1111111111" {
        if query.ends_with("level = 0") {
            assert_eq!(manager, None);
            Json(
                json!({"results": [{"customerClient": {"id": "1111111111", "descriptiveName": "Manager", "manager": true}}]}),
            )
        } else {
            assert_eq!(manager.as_deref(), Some("1111111111"));
            assert_eq!(query, "SELECT customer_client.id, customer_client.descriptive_name, customer_client.manager FROM customer_client WHERE customer_client.manager = FALSE AND customer_client.status = 'ENABLED'");
            if body.get("pageToken").is_none() {
                Json(
                    json!({"results": [{"customerClient": {"id": "2222222222", "descriptiveName": "Northwind Dental", "manager": false}}, {"customerClient": {"id": "5555555555", "descriptiveName": "Nested Manager", "manager": true}}], "nextPageToken": "page-2"}),
                )
            } else {
                assert_eq!(body["pageToken"], "page-2");
                Json(
                    json!({"results": [{"customerClient": {"id": "6666666666", "descriptiveName": "Lumen Yoga", "manager": false}}]}),
                )
            }
        }
    } else {
        assert_eq!(manager, None);
        Json(
            json!({"results": [{"customerClient": {"id": id, "descriptiveName": if id == "4444444444" {"Private B"} else {"Harbor Furniture"}, "manager": false}}]}),
        )
    };
    if empty_names {
        for row in response.0["results"].as_array_mut().unwrap() {
            row["customerClient"]["descriptiveName"] = json!("");
        }
    }
    (StatusCode::OK, response)
}

#[tokio::test]
#[serial]
async fn the_first_sign_in_with_google_starts_the_trial_at_that_moment() {
    let fixture = Fixture::new().await;
    let before = chrono::Utc::now().timestamp();
    fixture.sign_in("user-a").await;
    let after = chrono::Utc::now().timestamp();
    let trial = fixture
        .ctx
        .db
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            "SELECT started,started_at FROM workspace_trials".to_owned(),
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(trial.try_get::<i64>("", "started").unwrap(), 1);
    let started_at = trial.try_get::<i64>("", "started_at").unwrap();
    assert!(
        (before..=after).contains(&started_at),
        "{before} <= {started_at} <= {after}"
    );
}

#[tokio::test]
#[serial]
async fn sign_in_creates_and_reuses_user_workspace_and_hashes_sessions() {
    let fixture = Fixture::new().await;
    let (state, browser, url) = fixture.begin(None).await;
    let params: HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(params["scope"], "openid email profile");
    assert_eq!(
        params["redirect_uri"],
        "http://localhost:5150/auth/google/callback"
    );
    assert_eq!(params["client_id"], "stub-id");
    assert_eq!(params["response_type"], "code");
    assert_eq!(params["code_challenge_method"], "S256");
    assert_eq!(state.len(), 43);
    assert_eq!(browser.http_only(), Some(true));
    assert_eq!(browser.same_site(), Some(SameSite::Lax));
    assert_eq!(browser.max_age().unwrap().whole_seconds(), 600);
    let response = fixture.callback(&state, &browser, "user-a", None).await;
    assert_eq!(response.status_code(), 303, "{}", response.text());
    assert_eq!(response.header("location"), "/connect");
    let session = response.cookie(SESSION_COOKIE);
    assert_eq!(session.http_only(), Some(true));
    assert_eq!(session.same_site(), Some(SameSite::Lax));
    assert_ne!(session.secure(), Some(true));
    assert_eq!(session.path(), Some("/"));
    assert_eq!(session.max_age().unwrap().whole_seconds(), 2592000);
    {
        let calls = fixture.calls.lock().unwrap();
        assert_eq!(
            identity::hash_token(&calls.tokens[0]["code_verifier"]),
            params["code_challenge"]
        );
    }
    assert_eq!(fixture.count("users").await, 1);
    assert_eq!(fixture.count("workspaces").await, 1);
    assert_eq!(fixture.count("sessions").await, 1);
    let row = fixture
        .ctx
        .db
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            "SELECT token_hash, expires_at FROM sessions",
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.try_get::<String>("", "token_hash").unwrap(),
        identity::hash_token(session.value())
    );
    assert_ne!(
        row.try_get::<String>("", "token_hash").unwrap(),
        session.value()
    );
    assert!(row.try_get::<i64>("", "expires_at").unwrap() > chrono::Utc::now().timestamp());
    let signed = identity::authenticate(&fixture.ctx.db, session.value())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(signed.user.email, "a@example.com");
    assert_eq!(signed.user.name, "User A");
    let second = fixture.sign_in("user-a").await;
    let second_signed = identity::authenticate(&fixture.ctx.db, second.value())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second_signed.user.id, signed.user.id);
    assert_eq!(second_signed.workspace.id(), signed.workspace.id());
    assert_eq!(fixture.count("users").await, 1);
    assert_eq!(fixture.count("workspaces").await, 1);
    assert_eq!(fixture.count("sessions").await, 2);
    assert_eq!(fixture.count("google_connections").await, 0);
    assert_eq!(
        fixture
            .callback(&state, &browser, "user-a", None)
            .await
            .status_code(),
        400
    );
}

#[tokio::test]
#[serial]
async fn wrong_missing_expired_or_unbound_state_creates_nothing() {
    let fixture = Fixture::new().await;
    let (state, browser, _) = fixture.begin(None).await;
    assert_eq!(
        fixture
            .callback("wrong-state", &browser, "user-a", None)
            .await
            .status_code(),
        400
    );
    assert_eq!(
        fixture
            .request
            .get("/auth/google/callback?code=user-a")
            .add_cookie(browser.clone())
            .await
            .status_code(),
        400
    );
    assert_eq!(
        fixture
            .request
            .get("/auth/google/callback")
            .add_query_param("state", &state)
            .add_query_param("code", "user-a")
            .await
            .status_code(),
        400
    );
    assert_eq!(
        fixture
            .callback(
                &state,
                &Cookie::new(OAUTH_COOKIE, "another-browser"),
                "user-a",
                None
            )
            .await
            .status_code(),
        400
    );
    fixture
        .ctx
        .db
        .execute_unprepared("UPDATE oauth_states SET expires_at = 0")
        .await
        .unwrap();
    assert_eq!(
        fixture
            .callback(&state, &browser, "user-a", None)
            .await
            .status_code(),
        400
    );
    assert_eq!(fixture.count("users").await, 0);
    assert_eq!(fixture.count("workspaces").await, 0);
    assert_eq!(fixture.count("sessions").await, 0);
    assert_eq!(fixture.calls.lock().unwrap().tokens.len(), 0);
}

#[tokio::test]
#[serial]
async fn unsigned_requests_redirect_and_google_failures_are_clear() {
    let fixture = Fixture::new().await;
    for path in ["/ad-accounts", "/connect", "/ad-accounts/choose/unknown"] {
        let response = fixture.request.get(path).await;
        assert_eq!(response.status_code(), 303);
        assert_eq!(response.header("location"), "/sign-in");
    }
    let response = fixture
        .request
        .post("/connections/google/start")
        .add_header("origin", ORIGIN)
        .await;
    assert_eq!(response.status_code(), 303);
    assert_eq!(response.header("location"), "/sign-in");
    let (state, browser, _) = fixture.begin(None).await;
    let response = fixture.callback(&state, &browser, "denied", None).await;
    assert_eq!(response.status_code(), 502);
    assert!(response
        .text()
        .contains("Google could not complete this request. Please try again."));
    let (state, browser, _) = fixture.begin(None).await;
    assert_eq!(
        fixture
            .callback(&state, &browser, "unverified", None)
            .await
            .status_code(),
        502
    );
    assert_eq!(fixture.count("users").await, 0);
    assert_eq!(fixture.count("sessions").await, 0);
}

#[tokio::test]
#[serial]
async fn connect_encrypts_refresh_token_discovers_manager_clients_and_saves_selections() {
    let fixture = Fixture::new().await;
    let session = fixture.sign_in("user-a").await;
    let (state, browser, url) = fixture.begin(Some(&session)).await;
    let params: HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(
        params["scope"],
        "openid email https://www.googleapis.com/auth/adwords"
    );
    assert_eq!(params["access_type"], "offline");
    assert_eq!(params["prompt"], "consent");
    assert_eq!(
        params["redirect_uri"],
        "http://localhost:5150/auth/google/callback"
    );
    let response = fixture
        .callback(&state, &browser, "ads-a", Some(&session))
        .await;
    assert_eq!(response.status_code(), 303, "{}", response.text());
    let selection_path = response.header("location").to_str().unwrap().to_owned();
    let choices = fixture
        .request
        .get(&selection_path)
        .add_cookie(session.clone())
        .await;
    assert_eq!(choices.status_code(), 200);
    assert!(choices.text().contains("Northwind Dental"));
    assert!(choices.text().contains("Harbor Furniture"));
    assert!(choices.text().contains("Lumen Yoga"));
    assert!(!choices.text().contains("Nested Manager"));
    assert!(!choices.text().contains("value=\"1111111111\""));
    let row = fixture
        .ctx
        .db
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            "SELECT id, workspace_id, encrypted_refresh_token FROM google_connections",
        ))
        .await
        .unwrap()
        .unwrap();
    let encrypted = row
        .try_get::<String>("", "encrypted_refresh_token")
        .unwrap();
    let workspace_id = row.try_get::<String>("", "workspace_id").unwrap();
    let connection_id = row.try_get::<String>("", "id").unwrap();
    let key = MasterKey::from_base64(KEY).unwrap();
    assert_ne!(encrypted, "secret-refresh-token");
    assert_eq!(
        key.decrypt(&workspace_id, &connection_id, &encrypted)
            .unwrap(),
        "secret-refresh-token"
    );
    assert!(key
        .decrypt("other-workspace", &connection_id, &encrypted)
        .is_err());
    assert!(key.decrypt(&workspace_id, "other-row", &encrypted).is_err());
    assert_eq!(fixture.count("ad_accounts").await, 0);
    let invalid = fixture
        .request
        .post(&selection_path)
        .add_cookie(session.clone())
        .add_header("origin", ORIGIN)
        .form(&[("ad_account", "9999999999")])
        .await;
    assert_eq!(invalid.status_code(), 400);
    assert_eq!(fixture.count("ad_accounts").await, 0);
    let saved = fixture
        .request
        .post(&selection_path)
        .add_cookie(session.clone())
        .add_header("origin", ORIGIN)
        .form(&[("ad_account", "2222222222"), ("ad_account", "3333333333")])
        .await;
    assert_eq!(saved.status_code(), 303, "{}", saved.text());
    assert_eq!(saved.header("location"), "/ad-accounts");
    assert_eq!(fixture.count("ad_accounts").await, 2);
    let rows = fixture.ctx.db.query_all_raw(Statement::from_string(DatabaseBackend::Sqlite, "SELECT customer_id, name, login_customer_id, google_connection_id FROM ad_accounts ORDER BY customer_id")).await.unwrap();
    assert_eq!(
        rows[0].try_get::<String>("", "customer_id").unwrap(),
        "2222222222"
    );
    assert_eq!(
        rows[0].try_get::<String>("", "name").unwrap(),
        "Northwind Dental"
    );
    assert_eq!(
        rows[0]
            .try_get::<Option<String>>("", "login_customer_id")
            .unwrap(),
        Some("1111111111".into())
    );
    assert_eq!(
        rows[0]
            .try_get::<String>("", "google_connection_id")
            .unwrap(),
        connection_id
    );
    assert_eq!(
        rows[1]
            .try_get::<Option<String>>("", "login_customer_id")
            .unwrap(),
        None
    );
    assert_eq!(
        fixture
            .request
            .post(&selection_path)
            .add_cookie(session.clone())
            .add_header("origin", ORIGIN)
            .form(&[("ad_account", "2222222222")])
            .await
            .status_code(),
        400
    );
    fixture.connect(&session, "ads-a").await;
    assert_eq!(fixture.count("google_connections").await, 1);
    fixture.connect(&session, "ads-a-2").await;
    assert_eq!(fixture.count("google_connections").await, 2);
    let calls = fixture.calls.lock().unwrap();
    assert!(calls
        .ads
        .iter()
        .any(|(access, manager, body)| access == "Bearer ads-a"
            && manager.as_deref() == Some("1111111111")
            && body["pageToken"] == "page-2"));
}

#[tokio::test]
#[serial]
async fn workspace_b_cannot_read_or_add_workspace_a_connections_or_ad_accounts() {
    let fixture = Fixture::new().await;
    let session_a = fixture.sign_in("user-a").await;
    let selection_a = fixture.connect(&session_a, "ads-a").await;
    let session_b = fixture.sign_in("user-b").await;
    assert_eq!(
        fixture
            .request
            .get(&selection_a)
            .add_cookie(session_b.clone())
            .await
            .status_code(),
        404
    );
    assert_eq!(
        fixture
            .request
            .post(&selection_a)
            .add_cookie(session_b.clone())
            .add_header("origin", ORIGIN)
            .form(&[("ad_account", "2222222222")])
            .await
            .status_code(),
        400
    );
    let (state, browser, _) = fixture.begin(Some(&session_a)).await;
    assert_eq!(
        fixture
            .callback(&state, &browser, "ads-a", Some(&session_b))
            .await
            .status_code(),
        403
    );
    assert_eq!(fixture.count("google_connections").await, 1);
    let saved = fixture
        .request
        .post(&selection_a)
        .add_cookie(session_a.clone())
        .add_header("origin", ORIGIN)
        .form(&[("ad_account", "2222222222")])
        .await;
    assert_eq!(saved.status_code(), 303);
    assert!(fixture.ad_account_names(&session_b).await.is_empty());
    let selection_b = fixture.connect(&session_b, "ads-b").await;
    assert_eq!(
        fixture
            .request
            .post(&selection_b)
            .add_cookie(session_b.clone())
            .add_header("origin", ORIGIN)
            .form(&[("ad_account", "2222222222")])
            .await
            .status_code(),
        400
    );
    assert_eq!(
        fixture
            .request
            .post(&selection_b)
            .add_cookie(session_b.clone())
            .add_header("origin", ORIGIN)
            .form(&[("ad_account", "4444444444")])
            .await
            .status_code(),
        303
    );
    assert_eq!(
        fixture.ad_account_names(&session_a).await,
        ["Northwind Dental"]
    );
    assert_eq!(fixture.ad_account_names(&session_b).await, ["Private B"]);
    assert_eq!(fixture.count("users").await, 2);
    assert_eq!(fixture.count("workspaces").await, 2);
    assert_eq!(fixture.count("google_connections").await, 2);
    assert_eq!(fixture.count("ad_accounts").await, 2);
}

#[tokio::test]
#[serial]
async fn cross_origin_posts_are_refused_and_sign_out_ends_the_session() {
    let fixture = Fixture::new().await;
    let session = fixture.sign_in("user-a").await;
    let selection = fixture.connect(&session, "ads-a").await;
    for path in ["/sign-out", "/connections/google/start", selection.as_str()] {
        for origin in [
            "https://evil.example",
            "http://localhost:5151",
            "https://localhost:5150",
            "null",
        ] {
            assert_eq!(
                fixture
                    .request
                    .post(path)
                    .add_cookie(session.clone())
                    .add_header("origin", origin)
                    .await
                    .status_code(),
                403
            );
        }
        assert_eq!(
            fixture
                .request
                .post(path)
                .add_cookie(session.clone())
                .add_header("origin", ORIGIN)
                .add_header("sec-fetch-site", "cross-site")
                .await
                .status_code(),
            403
        );
        assert_eq!(
            fixture
                .request
                .post(path)
                .add_cookie(session.clone())
                .add_header("sec-fetch-site", "same-site")
                .await
                .status_code(),
            403
        );
        assert_eq!(
            fixture
                .request
                .post(path)
                .add_cookie(session.clone())
                .await
                .status_code(),
            403
        );
    }
    assert_eq!(fixture.count("sessions").await, 1);
    assert_eq!(fixture.count("ad_accounts").await, 0);
    let response = fixture
        .request
        .post("/sign-out")
        .add_cookie(session.clone())
        .add_header("sec-fetch-site", "same-origin")
        .await;
    assert_eq!(response.status_code(), 303);
    assert_eq!(response.header("location"), "/sign-in");
    assert_eq!(
        response
            .cookie(SESSION_COOKIE)
            .max_age()
            .unwrap()
            .whole_seconds(),
        0
    );
    assert_eq!(fixture.count("sessions").await, 0);
    let response = fixture
        .request
        .get("/ad-accounts")
        .add_cookie(session)
        .await;
    assert_eq!(response.status_code(), 303);
    assert_eq!(response.header("location"), "/sign-in");
}

#[tokio::test]
#[serial]
async fn missing_credentials_and_master_key_are_reported_without_panics() {
    let fixture = Fixture::configured(false, false, false).await;
    assert_eq!(fixture.request.get("/sign-in").await.status_code(), 200);
    let response = fixture.request.get("/auth/google/start").await;
    assert_eq!(response.status_code(), 503);
    assert!(response
        .text()
        .contains("Set GOOGLE_CLIENT_ID and GOOGLE_CLIENT_SECRET"));
    let mut ctx = fixture.ctx.clone();
    ctx.environment = Environment::Production;
    assert!(Settings::load(&ctx)
        .err()
        .unwrap()
        .to_string()
        .contains("KEYSEND_MASTER_KEY is required in production"));
    let failed_boot = App::boot(
        StartMode::WorkerOnly { tags: vec![] },
        &Environment::Production,
        fixture.ctx.config.clone(),
    )
    .await;
    assert!(failed_boot
        .err()
        .unwrap()
        .to_string()
        .contains("KEYSEND_MASTER_KEY is required in production"));
    assert!(MasterKey::from_base64("invalid-key").is_err());
    drop(fixture);
    let fixture = Fixture::configured(true, false, false).await;
    let session = fixture.sign_in("user-a").await;
    let response = fixture
        .request
        .post("/connections/google/start")
        .add_cookie(session)
        .add_header("origin", ORIGIN)
        .await;
    assert_eq!(response.status_code(), 503);
    assert!(response.text().contains("Set KEYSEND_MASTER_KEY"));
}

#[tokio::test]
#[serial]
async fn session_cookie_aliases_authenticate_without_public_caching() {
    let fixture = Fixture::new().await;
    let (state, browser, _) = fixture.begin(None).await;
    let response = fixture.callback(&state, &browser, "user-a", None).await;
    assert_eq!(response.status_code(), 303);
    let session = response.cookie(SESSION_COOKIE);
    for name in ["ks_session", "k%73_session", "ks_session "] {
        let cookie = format!("{name}={}", session.value());
        let accounts = fixture
            .request
            .get("/subscription")
            .add_header("cookie", cookie.clone())
            .await;
        assert_eq!(accounts.status_code(), 200, "{name}");
        for path in ["/", "/privacy", "/resources", "/static/styles.css"] {
            let response = fixture
                .request
                .get(path)
                .add_header("cookie", cookie.clone())
                .await;
            assert_eq!(response.status_code(), 200, "{name}: {path}");
            assert_eq!(
                response.header("cache-control"),
                "no-store",
                "{name}: {path}"
            );
        }
    }
}

#[tokio::test]
#[serial]
async fn https_sets_secure_cookies_and_expired_sessions_cannot_open_accounts() {
    let fixture = Fixture::configured(true, true, true).await;
    let (state, browser, url) = fixture.begin(None).await;
    assert_eq!(browser.secure(), Some(true));
    assert_eq!(
        url.query_pairs()
            .find(|(key, _)| key == "redirect_uri")
            .unwrap()
            .1,
        "https://keysend.example/auth/google/callback"
    );
    let response = fixture.callback(&state, &browser, "user-a", None).await;
    assert_eq!(response.status_code(), 303);
    let session = response.cookie(SESSION_COOKIE);
    assert_eq!(session.secure(), Some(true));
    fixture
        .ctx
        .db
        .execute_unprepared("UPDATE sessions SET expires_at = 0")
        .await
        .unwrap();
    let response = fixture
        .request
        .get("/ad-accounts")
        .add_cookie(session)
        .await;
    assert_eq!(response.status_code(), 303);
    assert_eq!(response.header("location"), "/sign-in");
}

#[derive(Clone)]
struct LogCapture(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for LogCapture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
#[serial]
async fn oauth_callback_queries_and_tokens_are_not_logged() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let writer = LogCapture(captured.clone());
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let _subscriber = tracing::subscriber::set_default(subscriber);
    let fixture = Fixture::new().await;
    let (state, browser, _) = fixture.begin(None).await;
    let response = fixture
        .callback(&state, &browser, "callback-secret-code", None)
        .await;
    assert_eq!(response.status_code(), 303);
    let session = response.cookie(SESSION_COOKIE);
    fixture.connect(&session, "ads-a").await;
    let (failed_state, failed_browser, _) = fixture.begin(None).await;
    let denied = fixture
        .callback(&failed_state, &failed_browser, "denied", None)
        .await;
    assert_eq!(denied.status_code(), 502);
    let logs = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
    assert!(
        logs.contains("http.uri=/auth/google/callback"),
        "No callback request was captured"
    );
    assert!(!logs.contains("callback-secret-code"));
    assert!(!logs.contains(&state));
    assert!(!logs.contains(browser.value()));
    assert!(!logs.contains(session.value()));
    assert!(!logs.contains("secret-refresh-token"));
    assert!(!logs.contains("Bearer ads-a"));
    assert!(!logs.contains("stub-secret"));
    let warning = logs
        .lines()
        .find(|line| line.contains("WARN") && line.contains("operation=\"token\""))
        .expect("No token failure warning was captured");
    assert!(warning.contains("http_status=400"));
    assert!(warning.contains("invalid_grant"));
    assert!(warning.contains("Rejected authorization:"));
    let message = warning
        .split("google_message=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    assert_eq!(message.chars().count(), 256);
    assert!(!logs.contains("END-OF-LONG-ERROR"));
    assert!(!message.contains("\\n"));
    assert!(!logs.contains("denied"));
    assert!(!logs.contains(&failed_state));
    assert!(!logs.contains(failed_browser.value()));
    for call in &fixture.calls.lock().unwrap().tokens {
        assert!(!logs.contains(&call["code_verifier"]));
    }
}

#[tokio::test]
#[serial]
async fn discovery_skips_unusable_customers_with_safe_warnings() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let writer = LogCapture(captured.clone());
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let _subscriber = tracing::subscriber::set_default(subscriber);
    let fixture = Fixture::new().await;
    let session = fixture.sign_in("user-a").await;
    {
        let mut calls = fixture.calls.lock().unwrap();
        calls.search_failure = Some(("7777777777".into(), StatusCode::FORBIDDEN));
    }
    let path = fixture.connect(&session, "ads-a").await;
    let page = fixture.request.get(&path).add_cookie(session.clone()).await;
    assert_eq!(page.status_code(), 200);
    for name in ["Northwind Dental", "Harbor Furniture", "Lumen Yoga"] {
        assert!(page.text().contains(name));
    }
    assert!(!page.text().contains("7777777777"));
    assert!(!page.text().contains("not-a-customer-id"));
    assert!(!page.text().contains("9999999999"));
    assert_eq!(fixture.count("ad_account_selections").await, 1);
    let logs = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
    let warnings: Vec<_> = logs
        .lines()
        .filter(|line| line.contains("WARN") && line.contains("operation=\"googleAds:search\""))
        .collect();
    assert_eq!(warnings.len(), 1);
    let warning = warnings[0];
    assert!(warning.contains("customer_id=\"7777777777\""));
    assert!(warning.contains("http_status=403"));
    assert!(warning.contains("PERMISSION_DENIED"));
    assert!(warning.contains("CUSTOMER_NOT_ENABLED"));
    assert!(warning.contains("Customer is unavailable:"));
    for secret in [
        "ads-a",
        "secret-refresh-token",
        "stub-secret",
        session.value(),
    ] {
        assert!(!warning.contains(secret));
    }
}

#[tokio::test]
#[serial]
async fn discovery_skips_malformed_accessible_resource_names() {
    let fixture = Fixture::new().await;
    let session = fixture.sign_in("user-a").await;
    fixture.calls.lock().unwrap().malformed_resources = true;
    let path = fixture.connect(&session, "ads-a").await;
    let page = fixture.request.get(&path).add_cookie(session).await;
    assert_eq!(page.status_code(), 200);
    assert!(page.text().contains("Northwind Dental"));
    assert!(page.text().contains("Harbor Furniture"));
    assert!(!page.text().contains("not-a-customer-id"));
    assert!(!page.text().contains("9999999999"));
}

#[tokio::test]
#[serial]
async fn discovery_skips_a_manager_when_its_client_search_fails() {
    let fixture = Fixture::new().await;
    let session = fixture.sign_in("user-a").await;
    fixture.calls.lock().unwrap().manager_search_failure = Some(StatusCode::FORBIDDEN);
    let path = fixture.connect(&session, "ads-a").await;
    let page = fixture.request.get(&path).add_cookie(session).await;
    assert_eq!(page.status_code(), 200);
    assert!(page.text().contains("Harbor Furniture"));
    assert!(!page.text().contains("Northwind Dental"));
    assert!(!page.text().contains("Lumen Yoga"));
    assert!(!page.text().contains("value=\"1111111111\""));
}

#[tokio::test]
#[serial]
async fn customer_search_transport_failure_fails_the_whole_discovery_without_secrets() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let captured = Arc::new(Mutex::new(Vec::new()));
    let writer = LogCapture(captured.clone());
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let _subscriber = tracing::subscriber::set_default(subscriber);
    let fixture = Fixture::new().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut settings = Settings::load(&fixture.ctx).unwrap();
    settings.google.ads_api_base = format!("http://{}/v25", listener.local_addr().unwrap());
    let stub = tokio::spawn(async move {
        let mut request = [0; 4096];
        let (mut socket, _) = listener.accept().await.unwrap();
        let bytes = socket.read(&mut request).await.unwrap();
        assert!(request[..bytes].starts_with(b"GET /v25/customers:listAccessibleCustomers "));
        let body = r#"{"resourceNames":["customers/3333333333"]}"#;
        let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        socket.write_all(response.as_bytes()).await.unwrap();
        socket.shutdown().await.unwrap();
        let (mut socket, _) = listener.accept().await.unwrap();
        let bytes = socket.read(&mut request).await.unwrap();
        assert!(request[..bytes].starts_with(b"POST /v25/customers/3333333333/googleAds:search "));
        socket.shutdown().await.unwrap();
    });
    let result = keysend::google::discover(&settings, "transport-secret-access-token").await;
    stub.await.unwrap();
    assert_eq!(
        result.err().unwrap().to_string(),
        "Google could not complete this request. Please try again."
    );
    let logs = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
    let warnings: Vec<_> = logs
        .lines()
        .filter(|line| line.contains("WARN") && line.contains("operation=\"googleAds:search\""))
        .collect();
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("customer_id=\"3333333333\""));
    assert!(warnings[0].contains("transport"));
    assert!(!logs.contains("transport-secret-access-token"));
    assert_eq!(fixture.count("ad_account_selections").await, 0);
}

#[tokio::test]
#[serial]
async fn accessible_customer_failure_is_generic_and_saves_no_selection() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let writer = LogCapture(captured.clone());
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let _subscriber = tracing::subscriber::set_default(subscriber);
    let fixture = Fixture::new().await;
    let session = fixture.sign_in("user-a").await;
    fixture.calls.lock().unwrap().accessible_failure = Some(StatusCode::FORBIDDEN);
    let (state, browser, _) = fixture.begin(Some(&session)).await;
    let response = fixture
        .callback(&state, &browser, "ads-a", Some(&session))
        .await;
    assert_eq!(response.status_code(), 502);
    assert!(response
        .text()
        .contains("Google could not complete this request. Please try again."));
    assert!(!response.text().contains("PROJECT_DISABLED"));
    assert_eq!(fixture.count("ad_account_selections").await, 0);
    assert_eq!(fixture.count("ad_accounts").await, 0);
    assert_eq!(fixture.calls.lock().unwrap().ads.len(), 1);
    let logs = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
    let warning = logs
        .lines()
        .find(|line| {
            line.contains("WARN")
                && line.contains("operation=\"customers:listAccessibleCustomers\"")
        })
        .expect("No accessible customer failure warning was captured");
    assert!(warning.contains("http_status=403"));
    assert!(warning.contains("PERMISSION_DENIED"));
    assert!(warning.contains("PROJECT_DISABLED"));
    assert!(!warning.contains("ads-a"));
}

#[tokio::test]
#[serial]
async fn customer_search_unauthorized_fails_the_whole_discovery() {
    let fixture = Fixture::new().await;
    let session = fixture.sign_in("user-a").await;
    fixture.calls.lock().unwrap().search_failure =
        Some(("7777777777".into(), StatusCode::UNAUTHORIZED));
    let (state, browser, _) = fixture.begin(Some(&session)).await;
    let response = fixture
        .callback(&state, &browser, "ads-a", Some(&session))
        .await;
    assert_eq!(response.status_code(), 502);
    assert!(response
        .text()
        .contains("Google could not complete this request. Please try again."));
    assert_eq!(fixture.count("ad_account_selections").await, 0);
}

#[tokio::test]
#[serial]
async fn empty_google_names_use_customer_ids_on_both_pages_and_in_storage() {
    let fixture = Fixture::new().await;
    let session = fixture.sign_in("user-a").await;
    fixture.calls.lock().unwrap().empty_names = true;
    let path = fixture.connect(&session, "ads-a").await;
    let page = fixture.request.get(&path).add_cookie(session.clone()).await;
    assert_eq!(page.status_code(), 200);
    for id in ["2222222222", "3333333333", "6666666666"] {
        assert!(page
            .text()
            .contains(&format!("<span>{id} <small class=\"num\">")));
    }
    let response = fixture
        .request
        .post(&path)
        .add_cookie(session.clone())
        .add_header("origin", ORIGIN)
        .form(&[("ad_account", "2222222222"), ("ad_account", "3333333333")])
        .await;
    assert_eq!(response.status_code(), 303);
    let rows = fixture
        .ctx
        .db
        .query_all_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            "SELECT customer_id, name FROM ad_accounts ORDER BY customer_id",
        ))
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].try_get::<String>("", "name").unwrap(), "2222222222");
    assert_eq!(rows[1].try_get::<String>("", "name").unwrap(), "3333333333");
}

#[tokio::test]
#[serial]
async fn sign_in_deletes_expired_sessions_and_keeps_active_sessions() {
    let fixture = Fixture::new().await;
    let expired = fixture.sign_in("user-a").await;
    fixture
        .ctx
        .db
        .execute_unprepared("UPDATE sessions SET expires_at = 0")
        .await
        .unwrap();
    let active = fixture.sign_in("user-b").await;
    assert_eq!(fixture.count("sessions").await, 1);
    let new = fixture.sign_in("user-a").await;
    assert_eq!(fixture.count("sessions").await, 2);
    assert!(identity::authenticate(&fixture.ctx.db, expired.value())
        .await
        .unwrap()
        .is_none());
    assert!(identity::authenticate(&fixture.ctx.db, active.value())
        .await
        .unwrap()
        .is_some());
    assert!(identity::authenticate(&fixture.ctx.db, new.value())
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
#[serial]
async fn development_starts_without_google_credentials() {
    let fixture = Fixture::configured(false, false, false).await;
    let boot = App::boot(
        StartMode::ServerOnly,
        &Environment::Development,
        fixture.ctx.config.clone(),
    )
    .await
    .unwrap();
    let request = TestServer::new(boot.router.unwrap()).unwrap();
    let page = request.get("/sign-in").await;
    assert_eq!(page.status_code(), 200);
    assert!(page.text().contains("Sign in with Google"));
    let response = request.get("/auth/google/start").await;
    assert_eq!(response.status_code(), 503);
    assert!(response
        .text()
        .contains("Set GOOGLE_CLIENT_ID and GOOGLE_CLIENT_SECRET"));
    assert_eq!(fixture.calls.lock().unwrap().tokens.len(), 0);
}

#[tokio::test]
#[serial]
async fn browser_form_post_with_null_origin_and_same_origin_metadata_is_accepted() {
    let fixture = Fixture::new().await;
    let session = fixture.sign_in("user-a").await;
    let response = fixture
        .request
        .post("/connections/google/start")
        .add_cookie(session.clone())
        .add_header("origin", "null")
        .add_header("sec-fetch-site", "same-origin")
        .await;
    assert_eq!(response.status_code(), 303);
    assert!(response
        .header("location")
        .to_str()
        .unwrap()
        .contains("scope=openid+email+https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fadwords"));
    assert_eq!(
        fixture
            .request
            .post("/connections/google/start")
            .add_cookie(session)
            .add_header("origin", "null")
            .add_header("sec-fetch-site", "cross-site")
            .await
            .status_code(),
        403
    );
}

#[tokio::test]
#[serial]
async fn add_ad_accounts_lists_them_again_without_a_new_consent() {
    let fixture = Fixture::new().await;
    let session = fixture.sign_in("user-a").await;
    let first = fixture.connect(&session, "ads-a").await;
    let saved = fixture
        .request
        .post(&first)
        .add_cookie(session.clone())
        .add_header("origin", ORIGIN)
        .form(&[("ad_account", "2222222222")])
        .await;
    assert_eq!(saved.status_code(), 303);
    let connection_id: String = fixture
        .ctx
        .db
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            "SELECT id FROM google_connections".to_owned(),
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "id")
        .unwrap();
    let path = format!("/connections/{connection_id}/ad-accounts");
    let refreshes = || -> Vec<std::collections::HashMap<String, String>> {
        fixture
            .calls
            .lock()
            .unwrap()
            .tokens
            .iter()
            .filter(|form| form["grant_type"] == "refresh_token")
            .cloned()
            .collect()
    };
    let before = refreshes().len();

    let response = fixture
        .request
        .post(&path)
        .add_cookie(session.clone())
        .add_header("origin", ORIGIN)
        .await;
    assert_eq!(response.status_code(), 303);
    let selection = response.header("location").to_str().unwrap().to_owned();
    assert!(selection.starts_with("/ad-accounts/choose/"));
    assert_ne!(selection, first);
    let refresh_calls = refreshes();
    assert_eq!(refresh_calls.len(), before + 1);
    assert_eq!(
        refresh_calls[before]["refresh_token"],
        "secret-refresh-token"
    );
    let page = fixture
        .request
        .get(&selection)
        .add_cookie(session.clone())
        .await;
    assert_eq!(page.status_code(), 200);
    assert!(page.text().contains("3333333333"));
    assert_eq!(page.text().matches("Already added").count(), 1);
    assert!(page
        .text()
        .contains(r#"value="2222222222" id="account-1" checked disabled"#));
    let saved = fixture
        .request
        .post(&selection)
        .add_cookie(session.clone())
        .add_header("origin", ORIGIN)
        .form(&[("ad_account", "3333333333")])
        .await;
    assert_eq!(saved.status_code(), 303);
    assert_eq!(fixture.count("ad_accounts").await, 2);
    assert_eq!(fixture.count("google_connections").await, 1);

    let other = fixture.sign_in("user-b").await;
    assert_eq!(
        fixture
            .request
            .post(&path)
            .add_cookie(other)
            .add_header("origin", ORIGIN)
            .await
            .status_code(),
        404
    );
    assert_eq!(
        fixture
            .request
            .post(&path)
            .add_header("origin", ORIGIN)
            .await
            .status_code(),
        303
    );

    fixture.calls.lock().unwrap().refresh_revoked = true;
    let revoked = fixture
        .request
        .post(&path)
        .add_cookie(session)
        .add_header("origin", ORIGIN)
        .await;
    assert_eq!(revoked.status_code(), 409);
    assert!(revoked.text().contains("Connect Google Ads again."));
    assert_eq!(fixture.count("ad_account_selections").await, 0);
}

#[tokio::test]
#[serial]
async fn a_new_user_sees_the_explanation_before_the_google_ads_consent() {
    let fixture = Fixture::new().await;
    let (state, browser, _) = fixture.begin(None).await;
    let first = fixture.callback(&state, &browser, "user-a", None).await;
    assert_eq!(first.header("location"), "/connect");
    let session = first.cookie(SESSION_COOKIE);

    let page = fixture
        .request
        .get("/connect")
        .add_cookie(session.clone())
        .await;
    assert_eq!(page.status_code(), 200);
    let text = page.text();
    assert!(text.contains(r#"<span class="caption">Step 2 of 4</span>Connect Google Ads"#));
    assert!(text.contains("Google now asks if KeySend can see your Google Ads. Choose Allow."));
    assert!(text.contains(
        "Google has no narrower permission to offer. We only read. We never change the Ad Account."
    ));
    assert!(text.contains(r#"action="/connections/google/start""#));
    assert!(!text.contains("cannot change"));

    fixture.connect(&session, "ads-a").await;
    let (state, browser, _) = fixture.begin(None).await;
    let again = fixture.callback(&state, &browser, "user-a", None).await;
    assert_eq!(again.header("location"), "/ad-accounts");
}

async fn pending_states(fixture: &Fixture) -> i64 {
    fixture
        .ctx
        .db
        .query_one_raw(sea_orm::Statement::from_string(
            sea_orm::DatabaseBackend::Sqlite,
            "SELECT COUNT(*) AS n FROM oauth_states",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "n")
        .unwrap()
}

#[tokio::test]
#[serial]
async fn a_browser_that_starts_again_replaces_its_pending_google_state() {
    let fixture = Fixture::new().await;
    let (first_state, cookie, _) = fixture.begin(None).await;
    let mut last_state = first_state.clone();
    for _ in 0..5 {
        let response = fixture
            .request
            .get("/auth/google/start")
            .add_cookie(cookie.clone())
            .await;
        assert_eq!(response.status_code(), 303);
        assert_eq!(response.cookie(OAUTH_COOKIE).value(), cookie.value());
        last_state = reqwest::Url::parse(response.header("location").to_str().unwrap())
            .unwrap()
            .query_pairs()
            .find(|(key, _)| key == "state")
            .unwrap()
            .1
            .into_owned();
    }
    assert_ne!(last_state, first_state);
    assert_eq!(pending_states(&fixture).await, 1);
}

#[tokio::test]
#[serial]
async fn a_full_set_of_pending_google_states_drops_the_oldest_and_sign_in_still_starts() {
    let fixture = Fixture::new().await;
    fixture
        .ctx
        .db
        .execute_unprepared(&format!("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<{}) INSERT INTO oauth_states(state_hash,browser_hash,pkce_verifier,expires_at) SELECT 'state'||i,'browser'||i,'verifier',strftime('%s','now')+600+i FROM n", keysend::identity::MAX_PENDING_STATES))
        .await
        .unwrap();
    let started = |fixture: &Fixture| {
        let request = fixture.request.get("/auth/google/start");
        async move { request.await.status_code() }
    };
    assert_eq!(started(&fixture).await, 303);
    assert_eq!(
        pending_states(&fixture).await,
        keysend::identity::MAX_PENDING_STATES as i64
    );
    let named = |name: &'static str| {
        let query = format!("SELECT COUNT(*) AS n FROM oauth_states WHERE state_hash='{name}'");
        let db = fixture.ctx.db.clone();
        async move {
            db.query_one_raw(sea_orm::Statement::from_string(
                sea_orm::DatabaseBackend::Sqlite,
                query,
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<i64>("", "n")
            .unwrap()
        }
    };
    assert_eq!(named("state1").await, 0);
    assert_eq!(named("state2").await, 1);
    assert_eq!(named("state1000").await, 1);
    assert_eq!(started(&fixture).await, 303);
    assert_eq!(
        pending_states(&fixture).await,
        keysend::identity::MAX_PENDING_STATES as i64
    );
    assert_eq!(named("state2").await, 1);
}

#[tokio::test]
#[serial]
async fn an_https_origin_sends_strict_transport_security() {
    let fixture = Fixture::configured(true, true, true).await;
    for path in ["/", "/sign-in", "/missing-page"] {
        let response = fixture.request.get(path).await;
        assert_eq!(
            response.header("strict-transport-security"),
            "max-age=31536000",
            "{path}"
        );
    }
}

impl Fixture {
    async fn sign_up_source(&self, email: &str) -> Option<String> {
        self.ctx
            .db
            .query_one_raw(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "SELECT w.sign_up_source AS source FROM workspaces w JOIN users u ON u.id=w.user_id WHERE u.email=?",
                vec![email.into()],
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get("", "source")
            .unwrap()
    }

    async fn sign_in_from(&self, code: &str, from: &str) -> reqwest::Url {
        let response = self
            .request
            .get("/auth/google/start")
            .add_query_param("from", from)
            .await;
        assert_eq!(response.status_code(), 303);
        let url = reqwest::Url::parse(response.header("location").to_str().unwrap()).unwrap();
        let state = url
            .query_pairs()
            .find(|(key, _)| key == "state")
            .unwrap()
            .1
            .into_owned();
        let browser = response.cookie(OAUTH_COOKIE);
        let response = self.callback(&state, &browser, code, None).await;
        assert_eq!(response.status_code(), 303, "{}", response.text());
        url
    }
}

#[tokio::test]
#[serial]
async fn a_new_workspace_keeps_the_public_button_that_led_to_the_sign_in() {
    let fixture = Fixture::new().await;
    let url = fixture.sign_in_from("user-a", "for-agency-clients").await;
    let params: HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert!(!params.contains_key("from"));
    assert_eq!(params["state"].len(), 43);
    assert!(!url.as_str().contains("for-agency-clients"));
    assert_eq!(
        fixture.sign_up_source("a@example.com").await.as_deref(),
        Some("for-agency-clients")
    );
    assert_eq!(fixture.count("oauth_states").await, 0);

    fixture.sign_in_from("user-a", "home-hero").await;
    assert_eq!(fixture.count("workspaces").await, 1);
    assert_eq!(
        fixture.sign_up_source("a@example.com").await.as_deref(),
        Some("for-agency-clients")
    );

    fixture.sign_in_from("user-b", "For-Agency-Clients").await;
    assert_eq!(fixture.sign_up_source("b@example.com").await, None);
}

#[tokio::test]
#[serial]
async fn a_sign_in_without_a_known_button_stores_no_source() {
    let fixture = Fixture::new().await;
    fixture.sign_in("user-a").await;
    assert_eq!(fixture.sign_up_source("a@example.com").await, None);
    fixture.sign_in_from("user-b", "home-hero\"><script>").await;
    assert_eq!(fixture.sign_up_source("b@example.com").await, None);
}
