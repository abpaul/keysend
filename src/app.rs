use std::{
    path::Path,
    sync::{Arc, OnceLock},
};

use async_trait::async_trait;
use axum::{
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};
use loco_rs::{
    app::{AppContext, Hooks, Initializer},
    bgworker::Queue,
    boot::{create_app, BootResult, StartMode},
    config::Config,
    controller::{
        views::{engines::TeraView, ViewRenderer},
        AppRoutes,
    },
    environment::Environment,
    task::Tasks,
    Result,
};
use migration::Migrator;
use tower_http::services::ServeDir;

use crate::{
    assets, controllers, initializers, security,
    settings::{Credentials, Settings},
};

pub struct App;

static VIEWS: OnceLock<TeraView> = OnceLock::new();

#[async_trait]
impl Hooks for App {
    fn app_name() -> &'static str {
        env!("CARGO_CRATE_NAME")
    }

    fn app_version() -> String {
        env!("CARGO_PKG_VERSION").to_owned()
    }

    async fn load_config(environment: &Environment) -> Result<Config> {
        let config = environment.load()?;
        Credentials::resolve(environment, &config)?;
        Ok(config)
    }

    async fn boot(
        mode: StartMode,
        environment: &Environment,
        config: Config,
    ) -> Result<BootResult> {
        Credentials::resolve(environment, &config)?;
        create_app::<Self, Migrator>(mode, environment, config).await
    }

    async fn after_context(mut ctx: AppContext) -> Result<AppContext> {
        let credentials = Credentials::resolve(&ctx.environment, &ctx.config)?;
        ctx.shared_store.insert(Arc::new(credentials));
        ctx.db = crate::database::connect(&ctx.config.database).await?;
        Ok(ctx)
    }

    async fn initializers(_ctx: &AppContext) -> Result<Vec<Box<dyn Initializer>>> {
        Ok(vec![Box::new(
            initializers::view_engine::ViewEngineInitializer,
        )])
    }

    fn routes(_ctx: &AppContext) -> AppRoutes {
        AppRoutes::with_default_routes()
            .add_route(controllers::content::routes())
            .add_route(controllers::page::routes())
            .add_route(controllers::auth::routes())
            .add_route(controllers::accounts::routes())
            .add_route(controllers::billing::routes())
            .add_route(controllers::control::routes())
            .add_route(controllers::monitoring::routes())
            .add_route(controllers::export::routes())
    }

    async fn after_routes(router: axum::Router, ctx: &AppContext) -> Result<axum::Router> {
        let settings = Arc::new(Settings::load(ctx)?);
        let content = Arc::new(crate::content::Catalog::load()?);
        let policy = security::Policy::new(
            settings.origin.clone(),
            serde_json::from_slice(&std::fs::read("config/cache-policy.json")?)?,
            std::env::var("KAMAL_VERSION")
                .ok()
                .filter(|version| {
                    version.len() == 40 && version.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
                .map(|version| version.parse().expect("validated release SHA")),
            content.clone(),
        );
        Ok(router
            .layer(axum::middleware::from_fn_with_state(
                settings.clone(),
                crate::error_reporting::observe,
            ))
            .layer(axum::Extension(settings))
            .layer(axum::Extension(content))
            .nest_service(
                "/static",
                ServeDir::new(assets::cache_dir(&ctx.environment))
                    .fallback(ServeDir::new("assets/static")),
            )
            .fallback(|| async { not_found() })
            .layer(axum::middleware::from_fn_with_state(
                policy,
                security::protect,
            )))
    }

    async fn connect_workers(ctx: &AppContext, queue: &Queue) -> Result<()> {
        use loco_rs::bgworker::BackgroundWorker;
        queue
            .register(crate::monitoring::worker::MonitoringWorker::build(ctx))
            .await?;
        queue
            .register(crate::monitoring::worker::EmailWorker::build(ctx))
            .await?;
        queue
            .register(crate::monitoring::worker::TranslationWorker::build(ctx))
            .await?;
        queue
            .register(crate::monitoring::retention::RetentionWorker::build(ctx))
            .await?;
        if ctx.environment == Environment::Production {
            tokio::spawn(crate::monitoring::schedule::hourly(ctx.clone()));
        }
        Ok(())
    }

    fn register_tasks(tasks: &mut Tasks) {
        tasks.register(crate::billing::setup::StripeSetup);
    }

    async fn truncate(_ctx: &AppContext) -> Result<()> {
        Ok(())
    }

    async fn seed(_ctx: &AppContext, _base: &Path) -> Result<()> {
        Ok(())
    }
}

pub fn keep_views(views: TeraView) {
    let _ = VIEWS.set(views);
}

pub fn not_found() -> Response {
    match VIEWS
        .get()
        .expect("views loaded at startup")
        .render("not_found.html", serde_json::json!({}))
    {
        Ok(html) => (StatusCode::NOT_FOUND, Html(html)).into_response(),
        Err(error) => {
            tracing::error!(%error, "The not-found page did not render");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
