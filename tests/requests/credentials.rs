use std::{
    collections::HashMap,
    env,
    sync::{Arc, Mutex},
};

use axum::{
    extract::{Form, State},
    http::HeaderMap,
    routing::{get, post},
    Json, Router,
};
use axum_test::TestServer;
use keysend::{
    app::App,
    auth::{OAUTH_COOKIE, SESSION_COOKIE},
};
use loco_rs::{
    app::Hooks,
    boot::{BootResult, StartMode},
    environment::Environment,
    Result,
};
use serde_json::{json, Value};
use serial_test::serial;

const UNLOCK_KEY: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";
const OTHER_UNLOCK_KEY: &str = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=";
const DATA_KEY: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
const ORIGIN: &str = "http://localhost:5150";
const SEALED: &str = "tests/fixtures/credentials/sealed";
const NAMES: [&str; 10] = [
    "CREDSEAL_MASTER_KEY",
    "POSTDEPLOY_INGEST_KEY",
    "STRIPE_SECRET_KEY",
    "STRIPE_WEBHOOK_SECRET",
    "OPENAI_API_KEY",
    "CLOUDFLARE_EMAIL_TOKEN",
    "REPORT_EMAIL_RECIPIENT",
    "GOOGLE_CLIENT_ID",
    "GOOGLE_CLIENT_SECRET",
    "KEYSEND_MASTER_KEY",
];

type TokenForms = Arc<Mutex<Vec<HashMap<String, String>>>>;

struct EnvironmentGuard(Vec<(&'static str, Option<String>)>);

impl EnvironmentGuard {
    fn new(values: &[(&str, &str)]) -> Self {
        let previous = NAMES.map(|name| (name, env::var(name).ok())).to_vec();
        for name in NAMES {
            match values.iter().find(|(set, _)| *set == name) {
                Some((_, value)) => env::set_var(name, value),
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

fn conflicting_environment(unlock_key: Option<&'static str>) -> EnvironmentGuard {
    let mut values = vec![
        ("GOOGLE_CLIENT_ID", "environment-google-client-id"),
        ("GOOGLE_CLIENT_SECRET", "environment-google-client-secret"),
        ("KEYSEND_MASTER_KEY", DATA_KEY),
    ];
    if let Some(unlock_key) = unlock_key {
        values.push(("CREDSEAL_MASTER_KEY", unlock_key));
        values.push(("KEYSEND_MASTER_KEY", "not-a-data-key"));
        values.push(("STRIPE_SECRET_KEY", "not-a-stripe-key"));
    }
    EnvironmentGuard::new(&values)
}

async fn boot(environment: &Environment, directory: &str, google: &str) -> Result<BootResult> {
    let mut config = Environment::Test.load()?;
    config.database.uri = "sqlite::memory:".into();
    config.queue = None;
    config.settings = Some(json!({
        "credentials_directory": directory,
        "google": {
            "authorization_url": format!("{google}/authorize"),
            "token_url": format!("{google}/token"),
            "userinfo_url": format!("{google}/userinfo"),
        },
    }));
    App::boot(StartMode::ServerOnly, environment, config).await
}

fn start_error(result: Result<BootResult>) -> String {
    match result {
        Ok(_) => panic!("KeySend started"),
        Err(error) => error.to_string(),
    }
}

async fn google() -> (String, TokenForms) {
    async fn token(
        State(forms): State<TokenForms>,
        Form(form): Form<HashMap<String, String>>,
    ) -> Json<Value> {
        let code = form["code"].clone();
        forms.lock().unwrap().push(form);
        Json(json!({"access_token": code, "token_type": "Bearer"}))
    }
    async fn userinfo(_: HeaderMap) -> Json<Value> {
        Json(json!({
            "sub": "subject-a", "email": "a@example.com", "name": "User A", "email_verified": true,
        }))
    }
    let forms = Arc::new(Mutex::new(Vec::new()));
    let router = Router::new()
        .route("/token", post(token))
        .route("/userinfo", get(userinfo))
        .with_state(forms.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (url, forms)
}

struct SignIn {
    client_id: String,
    token_client_id: String,
    token_client_secret: String,
    connection_status: u16,
}

async fn sign_in_with_google(directory: &str) -> SignIn {
    let (google, forms) = google().await;
    let boot = boot(&Environment::Test, directory, &google).await.unwrap();
    let request = TestServer::new(boot.router.unwrap()).unwrap();
    let start = request.get("/auth/google/start").await;
    assert_eq!(start.status_code(), 303);
    let authorization = reqwest::Url::parse(start.header("location").to_str().unwrap()).unwrap();
    let query: HashMap<String, String> = authorization.query_pairs().into_owned().collect();
    let callback = request
        .get("/auth/google/callback")
        .add_query_param("state", &query["state"])
        .add_query_param("code", "user-a")
        .add_cookie(start.cookie(OAUTH_COOKIE))
        .await;
    assert_eq!(callback.status_code(), 303, "{}", callback.text());
    let connection = request
        .post("/connections/google/start")
        .add_header("origin", ORIGIN)
        .add_cookie(callback.cookie(SESSION_COOKIE))
        .await;
    let token = forms.lock().unwrap().pop().unwrap();
    SignIn {
        client_id: query["client_id"].clone(),
        token_client_id: token["client_id"].clone(),
        token_client_secret: token["client_secret"].clone(),
        connection_status: connection.status_code().as_u16(),
    }
}

#[tokio::test]
#[serial]
async fn a_user_signs_in_with_the_sealed_google_credentials_and_not_the_environment() {
    let _environment = conflicting_environment(Some(UNLOCK_KEY));
    let sign_in = sign_in_with_google(SEALED).await;
    assert_eq!(sign_in.client_id, "sealed-google-client-id");
    assert_eq!(sign_in.token_client_id, "sealed-google-client-id");
    assert_eq!(sign_in.token_client_secret, "sealed-google-client-secret");
    assert_eq!(sign_in.connection_status, 303);
}

#[tokio::test]
#[serial]
async fn a_user_signs_in_with_the_environment_credentials_when_no_unlock_key_is_set() {
    let _environment = conflicting_environment(None);
    let sign_in = sign_in_with_google(SEALED).await;
    assert_eq!(sign_in.client_id, "environment-google-client-id");
    assert_eq!(sign_in.token_client_id, "environment-google-client-id");
    assert_eq!(
        sign_in.token_client_secret,
        "environment-google-client-secret"
    );
    assert_eq!(sign_in.connection_status, 303);
}

#[tokio::test]
#[serial]
async fn sealed_credentials_that_cannot_be_read_stop_the_start_without_showing_a_secret() {
    let tampered = env::temp_dir().join(format!(
        "keysend-credentials-{}",
        keysend::identity::random_token()
    ));
    std::fs::create_dir_all(&tampered).unwrap();
    let mut file = std::fs::read(format!("{SEALED}/test.toml.enc")).unwrap();
    let last = file.len() - 4;
    file[last] = if file[last] == b'A' { b'B' } else { b'A' };
    std::fs::write(tampered.join("test.toml.enc"), file).unwrap();
    let tampered = tampered.display().to_string();
    let decrypt =
        "cannot decrypt credentials: wrong key, wrong application or environment, or modified file";
    let cases = [
        (
            "",
            SEALED,
            format!("Cannot load the credentials in {SEALED}/test.toml.enc: the master key is not Base64 of exactly 32 bytes"),
        ),
        (
            OTHER_UNLOCK_KEY,
            SEALED,
            format!("Cannot load the credentials in {SEALED}/test.toml.enc: {decrypt}"),
        ),
        (
            UNLOCK_KEY,
            &tampered,
            format!("Cannot load the credentials in {tampered}/test.toml.enc: {decrypt}"),
        ),
        (
            UNLOCK_KEY,
            "tests/fixtures/credentials/unknown-field",
            "Cannot load the credentials in tests/fixtures/credentials/unknown-field/test.toml.enc: credentials do not match the expected type (line 2)".into(),
        ),
        (
            UNLOCK_KEY,
            "tests/fixtures/credentials/missing",
            "Cannot load the credentials in tests/fixtures/credentials/missing/test.toml.enc: cannot open tests/fixtures/credentials/missing/test.toml.enc: No such file or directory (os error 2)".into(),
        ),
    ];
    let mut errors = Vec::new();
    for (unlock_key, directory, _) in &cases {
        let _environment = conflicting_environment(Some(unlock_key));
        errors.push(start_error(
            boot(&Environment::Test, directory, "http://127.0.0.1:9").await,
        ));
    }
    std::fs::remove_dir_all(&tampered).unwrap();
    for (error, (_, _, expected)) in errors.iter().zip(&cases) {
        assert_eq!(error, expected);
        assert!(!error.contains("sealed-"), "{error}");
        assert!(!error.contains("misspelled"), "{error}");
        assert!(!error.contains(UNLOCK_KEY), "{error}");
    }
    let _environment = conflicting_environment(Some(UNLOCK_KEY));
    assert_eq!(
        App::load_config(&Environment::Test)
            .await
            .err()
            .unwrap()
            .to_string(),
        "Cannot load the credentials in config/credentials/test.toml.enc: cannot open config/credentials/test.toml.enc: No such file or directory (os error 2)"
    );
}

#[tokio::test]
#[serial]
async fn production_does_not_start_without_a_sealed_data_key() {
    let _environment = conflicting_environment(Some(UNLOCK_KEY));
    env::set_var("KEYSEND_MASTER_KEY", DATA_KEY);
    env::remove_var("STRIPE_SECRET_KEY");
    assert_eq!(
        start_error(
            boot(
                &Environment::Production,
                "tests/fixtures/credentials/without-data-key",
                "http://127.0.0.1:9",
            )
            .await
        ),
        "KEYSEND_MASTER_KEY is required in production"
    );
}
