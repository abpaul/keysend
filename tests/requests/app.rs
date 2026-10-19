use std::{
    env,
    sync::{Arc, Mutex},
};

use axum::{extract::State, routing::post, Json, Router};
use axum_extra::extract::cookie::Cookie;
use axum_test::TestServer;
use keysend::{
    app::App,
    auth::SESSION_COOKIE,
    crypto::MasterKey,
    identity,
    workspace::{AdAccountChoice, WorkspaceStore},
};
use loco_rs::{
    app::{AppContext, Hooks},
    bgworker,
    boot::{create_context, run_app, BootResult, StartMode},
    config::Config,
    db,
    environment::Environment,
};
use migration::Migrator;
use serde_json::{json, Value};
use serial_test::serial;
use tokio::task::JoinHandle;

const REAL_AI_KEY: &str = "KEYSEND_REAL_AI_KEY";
const ORIGIN: &str = "http://localhost:5150";
const KEY: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

mod billing;
mod control;
mod demo;
mod google_ads;
mod google_stub;
mod monitoring;
mod perf;
mod public;
mod retention;
mod stripe_stub;

async fn boot(mode: StartMode, config: Config, statements: Arc<Mutex<Vec<String>>>) -> BootResult {
    let environment = Environment::Test;
    let mut ctx = create_context::<App>(&environment, config).await.unwrap();
    ctx.db.set_metric_callback(move |info| {
        statements.lock().unwrap().push(info.statement.sql.clone());
    });
    db::converge::<App, Migrator>(&ctx, &ctx.config.database)
        .await
        .unwrap();
    if let (Some(queue), Some(config)) = (&ctx.queue_provider, &ctx.config.queue) {
        bgworker::converge(queue, config).await.unwrap();
    }
    run_app::<App>(&mode, ctx).await.unwrap()
}

#[derive(Default)]
struct Calls {
    errors: Vec<Value>,
    commentary: Vec<Value>,
    research: Vec<Value>,
    translations: Vec<Value>,
    translation_answer: Option<fn(&str, &str) -> String>,
    emails: Vec<Value>,
    invented: bool,
    override_monitoring: Option<Value>,
    override_recommendations: Option<Value>,
    email_refused: bool,
    stripe: stripe_stub::Calls,
    google: google_stub::Calls,
}

struct Fixture {
    request: TestServer,
    router: Router,
    ctx: AppContext,
    a: Cookie<'static>,
    b: Cookie<'static>,
    calls: Arc<Mutex<Calls>>,
    statements: Arc<Mutex<Vec<String>>>,
    stub: JoinHandle<()>,
    environment: Vec<(&'static str, Option<String>)>,
    database: std::path::PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stub.abort();
        for suffix in ["", "-wal", "-shm"] {
            std::fs::remove_file(format!("{}{suffix}", self.database.display())).ok();
        }
        for (name, value) in &self.environment {
            match value {
                Some(value) => env::set_var(name, value),
                None => env::remove_var(name),
            }
        }
    }
}

impl Fixture {
    async fn new(invented: bool) -> Self {
        Self::configured(invented, false, true).await
    }

    async fn configured(invented: bool, queued: bool, payments: bool) -> Self {
        Self::configured_source(invented, queued, payments, "sample").await
    }

    async fn configured_source(invented: bool, queued: bool, payments: bool, source: &str) -> Self {
        Self::configured_source_limit(invented, queued, payments, source, 15_000).await
    }

    async fn configured_source_limit(
        invented: bool,
        queued: bool,
        payments: bool,
        source: &str,
        daily_limit: u32,
    ) -> Self {
        Self::configured_clock(
            invented,
            queued,
            payments,
            source,
            daily_limit,
            "2026-10-11T12:00:00Z",
        )
        .await
    }

    async fn configured_clock(
        invented: bool,
        queued: bool,
        payments: bool,
        source: &str,
        daily_limit: u32,
        test_now: &str,
    ) -> Self {
        let clock = (!queued).then_some(test_now);
        let stripe_key = if payments {
            "sk_test_stub-stripe-secret"
        } else {
            ""
        };
        Self::built(invented, queued, stripe_key, source, daily_limit, clock).await
    }

    async fn live_payments() -> Self {
        let f = Self::built(
            false,
            false,
            "rk_live_stub-stripe-secret",
            "sample",
            15_000,
            Some("2026-10-11T12:00:00Z"),
        )
        .await;
        f.calls.lock().unwrap().stripe.live = true;
        f
    }

    async fn queued_at(test_now: &str) -> Self {
        let f = Self::built(
            false,
            true,
            "sk_test_stub-stripe-secret",
            "google",
            15_000,
            Some(test_now),
        )
        .await;
        let queue = f.ctx.queue_provider.clone().unwrap();
        tokio::spawn(async move { queue.run(Vec::new()).await });
        f
    }

    async fn built(
        invented: bool,
        queued: bool,
        stripe_key: &str,
        source: &str,
        daily_limit: u32,
        test_now: Option<&str>,
    ) -> Self {
        let real_ai_key = env::var(REAL_AI_KEY).ok();
        let mut environment = vec![("CREDSEAL_MASTER_KEY", env::var("CREDSEAL_MASTER_KEY").ok())];
        env::remove_var("CREDSEAL_MASTER_KEY");
        for (name, value) in [
            ("POSTDEPLOY_INGEST_KEY", "stub-postdeploy-secret"),
            (
                "OPENAI_API_KEY",
                real_ai_key.as_deref().unwrap_or("stub-openai-secret"),
            ),
            ("CLOUDFLARE_EMAIL_TOKEN", "stub-email-secret"),
            ("REPORT_EMAIL_RECIPIENT", "a@example.com"),
            ("KEYSEND_MASTER_KEY", KEY),
            ("GOOGLE_CLIENT_ID", "google-client-id"),
            ("GOOGLE_CLIENT_SECRET", "google-client-secret"),
            ("STRIPE_SECRET_KEY", stripe_key),
            (
                "STRIPE_WEBHOOK_SECRET",
                if stripe_key.is_empty() {
                    ""
                } else {
                    "whsec_stub-stripe-secret"
                },
            ),
        ] {
            environment.push((name, env::var(name).ok()));
            env::set_var(name, value);
        }
        let calls = Arc::new(Mutex::new(Calls {
            invented,
            ..Default::default()
        }));
        async fn commentary(
            State(calls): State<Arc<Mutex<Calls>>>,
            Json(body): Json<Value>,
        ) -> Json<Value> {
            let mut calls = calls.lock().unwrap();
            let format_name = body["text"]["format"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            if format_name == "monitoring_translation" {
                let input: Value = serde_json::from_str(body["input"].as_str().unwrap()).unwrap();
                let language = input["language"].as_str().unwrap().to_owned();
                let answer = calls
                    .translation_answer
                    .unwrap_or(|language, text| format!("[{language}] {text}"));
                let items = input["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|item| {
                        json!({"id": item["id"], "text": answer(&language, item["text"].as_str().unwrap())})
                    })
                    .collect::<Vec<_>>();
                calls.translations.push(body);
                return Json(
                    json!({"output":[{"type":"message","content":[{"type":"output_text","text":json!({"items":items}).to_string()}]}]}),
                );
            }
            if format_name == "business_research" {
                calls.research.push(body);
                return Json(
                    json!({"output":[{"type":"message","content":[{"type":"output_text","text":json!({"found":false,"kind":"other","summary":"","offers":[],"areas":[],"facts":[]}).to_string()}]}]}),
                );
            }
            if format_name == "monitoring_update" || format_name == "monitoring_review" {
                let input: Value = serde_json::from_str(body["input"].as_str().unwrap()).unwrap();
                calls.commentary.push(body);
                let override_answer = if format_name == "monitoring_review" {
                    calls
                        .override_recommendations
                        .as_ref()
                        .or(calls.override_monitoring.as_ref())
                } else {
                    calls.override_monitoring.as_ref()
                };
                let answer = if let Some(answer) = override_answer {
                    answer.clone()
                } else if format_name == "monitoring_update" {
                    let summary = if calls.invented {
                        "Spend was 999999.".to_owned()
                    } else {
                        format!(
                            "In the last 7 days, spend was {} against {}.",
                            input["metrics"]["current"]["Spend"].as_str().unwrap(),
                            input["metrics"]["previous"]["Spend"].as_str().unwrap()
                        )
                    };
                    let lines=input["campaigns"].as_array().unwrap().iter().map(|campaign|json!({"campaign_id":campaign["campaign_id"],"text":format!("Overall, {} spent {} against {}.",campaign["name"].as_str().unwrap(),campaign["metrics"]["current"]["Spend"].as_str().unwrap(),campaign["metrics"]["previous"]["Spend"].as_str().unwrap())})).collect::<Vec<_>>();
                    json!({"summary":summary,"lines":lines})
                } else {
                    let recommendations=input["findings"].as_array().unwrap().iter().filter(|finding|finding["recommendation_allowed"]==true).map(|finding|json!({"finding":finding["key"],"intent":"investigate","action":format!("Review {}.",finding["subjects"][0].as_str().unwrap()),"reason":"Its cost is high compared with the Ad Account."})).collect::<Vec<_>>();
                    json!({"recommendations":recommendations})
                };
                return Json(
                    json!({"output":[{"type":"message","content":[{"type":"output_text","text":answer.to_string()}]}]}),
                );
            }
            panic!("Unexpected AI request: {format_name}");
        }
        async fn email(
            State(calls): State<Arc<Mutex<Calls>>>,
            Json(body): Json<Value>,
        ) -> Json<Value> {
            let mut calls = calls.lock().unwrap();
            calls.emails.push(body);
            Json(json!({"success":!calls.email_refused}))
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        async fn error_ingest(
            State(calls): State<Arc<Mutex<Calls>>>,
            headers: axum::http::HeaderMap,
            Json(body): Json<Value>,
        ) -> (axum::http::StatusCode, Json<Value>) {
            assert_eq!(headers["authorization"], "Bearer stub-postdeploy-secret");
            calls.lock().unwrap().errors.push(body);
            (
                axum::http::StatusCode::ACCEPTED,
                Json(json!({"accepted":1,"dropped":0})),
            )
        }
        let router = Router::new()
            .route("/errors", post(error_ingest))
            .route("/commentary", post(commentary))
            .route("/email", post(email))
            .with_state(calls.clone())
            .merge(stripe_stub::routes(calls.clone()));
        let router = router.merge(google_stub::routes(calls.clone()));
        let stub = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let mut config = App::load_config(&Environment::Test).await.unwrap();
        let database = std::env::temp_dir().join(format!(
            "keysend-app-{}-{}.sqlite",
            if queued { "queue" } else { "request" },
            identity::random_token()
        ));
        config.database.uri = format!("sqlite://{}?mode=rwc", database.display());
        if queued {
            config.workers.mode = loco_rs::config::WorkerMode::BackgroundQueue;
            let Some(loco_rs::config::QueueConfig::Sqlite(queue)) = config.queue.as_mut() else {
                panic!("SQLite queue is required");
            };
            queue.uri = config.database.uri.clone();
            queue.num_workers = 4;
            queue.poll_interval_sec = 1;
        } else {
            config.queue = None;
        }
        config.settings = Some(
            json!({"monitoring":{"first_update_wait_seconds":3},"postdeploy":{"url":format!("{base}/errors")},"services":{"ai_model":env::var("KEYSEND_REAL_AI_MODEL").unwrap_or_else(|_| "gpt-6.1-sol".into()),"ai_url":if real_ai_key.is_some() {"https://api.openai.com/v1/responses".to_owned()} else {format!("{base}/commentary")},"email_url":format!("{base}/email"),"test_now":test_now},"billing":{"api_base":format!("{base}/stripe")},"google":{"revoke_url":format!("{base}/google/revoke"),"token_url":format!("{base}/google/token"),"ads_api_base":format!("{base}/google/v25"),"daily_limit":daily_limit}}),
        );
        let statements = Arc::new(Mutex::new(Vec::new()));
        let boot = boot(
            if queued {
                StartMode::ServerAndWorker
            } else {
                StartMode::ServerOnly
            },
            config,
            statements.clone(),
        )
        .await;
        let ctx = boot.app_context;
        let now = keysend::settings::Settings::load(&ctx)
            .unwrap()
            .now()
            .timestamp();
        let token = identity::sign_in(&ctx.db, "a", "a@example.com", "User A", now, None)
            .await
            .unwrap();
        let a = Cookie::new(SESSION_COOKIE, token.clone());
        let signed = identity::authenticate(&ctx.db, &token)
            .await
            .unwrap()
            .unwrap();
        let store = WorkspaceStore::new(&ctx.db, &signed.workspace);
        let connection = store
            .save_google_connection(
                "a",
                "a@example.com",
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
                    name: "Northwind Dental".into(),
                    login_customer_id: if source == "google" {
                        Some("9911882277".into())
                    } else {
                        None
                    },
                }],
            )
            .await
            .unwrap();
        assert!(store
            .save_selected(&selection, &["4829153307".into()])
            .await
            .unwrap());
        let b = Cookie::new(
            SESSION_COOKIE,
            identity::sign_in(&ctx.db, "b", "b@example.com", "User B", now, None)
                .await
                .unwrap(),
        );
        let router = boot.router.unwrap();
        Self {
            request: TestServer::new(router.clone()).unwrap(),
            router,
            ctx,
            a,
            b,
            calls,
            statements,
            stub,
            environment,
            database,
        }
    }

    async fn page(&self, path: &str) -> String {
        let response = self.request.get(path).add_cookie(self.a.clone()).await;
        assert_eq!(response.status_code(), 200, "{}", response.text());
        response.text()
    }

    async fn post(
        &self,
        cookie: &Cookie<'static>,
        route: &str,
        form: Value,
    ) -> axum_test::TestResponse {
        self.request
            .post(route)
            .add_header("origin", ORIGIN)
            .add_cookie(cookie.clone())
            .form(&form)
            .await
    }
}

fn capture() -> (Arc<Mutex<Vec<u8>>>, tracing::subscriber::DefaultGuard) {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let writer = LogCapture(captured.clone());
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    (captured, tracing::subscriber::set_default(subscriber))
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

/// The strings of an Export source, one on each line, with the pieces of a sentence joined.
fn typst_text(source: &str) -> String {
    let source = source.replace("\")#campaign(\"", "").replace("\")#(\"", "");
    let mut out = String::new();
    let mut rest = source.as_str();
    while let Some(start) = rest.find('"') {
        let tail = &rest[start + 1..];
        let mut end = None;
        let mut escaped = false;
        for (index, character) in tail.char_indices() {
            match character {
                '\\' if !escaped => escaped = true,
                '"' if !escaped => {
                    end = Some(index);
                    break;
                }
                _ => escaped = false,
            }
        }
        let Some(end) = end else { break };
        let literal = format!("\"{}\"", &tail[..end]);
        let string = serde_json::from_str::<String>(&literal).unwrap_or_default();
        if !string.ends_with(".svg") && !string.ends_with(".png") {
            out.push_str(&string);
            out.push('\n');
        }
        rest = &tail[end + 1..];
    }
    out
}

/// Gets an Export from its route and gives the words that its PDF prints: the route must serve
/// the PDF of the Export that the controller builds.
async fn export_words(
    f: &Fixture,
    signed: &identity::SignedIn,
    ad: &str,
    campaign: Option<i64>,
    window: &str,
) -> String {
    use keysend::{
        controllers::export::{ad_account_export, campaign_export, Params},
        monitoring::export,
    };
    let path = match campaign {
        Some(campaign) => {
            format!("/ad-accounts/{ad}/campaigns/{campaign}/export.pdf?window={window}")
        }
        None => format!("/ad-accounts/{ad}/export.pdf?window={window}"),
    };
    let response = f.request.get(&path).add_cookie(f.a.clone()).await;
    assert_eq!(response.status_code(), 200, "{path}");
    let store = WorkspaceStore::new(&f.ctx.db, &signed.workspace);
    let settings = keysend::settings::Settings::load(&f.ctx).unwrap();
    let params = Params {
        window: Some(window.into()),
    };
    let language = signed.user.language;
    let built = match campaign {
        Some(campaign) => campaign_export(&store, &settings, ad, campaign, &params, language).await,
        None => ad_account_export(&store, &settings, ad, &params, language).await,
    }
    .unwrap();
    let Ok((export, text)) = built else {
        panic!("{path} is refused");
    };
    assert!(
        response.as_bytes() == export::pdf(&export, text).unwrap().as_slice(),
        "{path} serves another PDF"
    );
    typst_text(&export::source(&export, text).unwrap())
}
