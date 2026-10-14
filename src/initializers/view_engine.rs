use async_trait::async_trait;
use axum::{Extension, Router};
use loco_rs::{
    app::{AppContext, Initializer},
    controller::views::{engines, ViewEngine},
    Result,
};

pub struct ViewEngineInitializer;

#[async_trait]
impl Initializer for ViewEngineInitializer {
    fn name(&self) -> String {
        "view-engine".to_owned()
    }

    async fn after_routes(&self, router: Router, ctx: &AppContext) -> Result<Router> {
        let assets = crate::assets::prepare(
            std::path::Path::new("assets/static"),
            &crate::assets::cache_dir(&ctx.environment),
        )?;
        let engine = engines::TeraView::build_with_post_process(move |tera| {
            crate::language::register(tera);
            tera.global_context().insert("assets", &assets);
            Ok(())
        })?;
        crate::app::keep_views(engine.clone());
        Ok(router
            .layer(Extension(ViewEngine::from(engine)))
            .layer(axum::middleware::from_fn(crate::language::scope_request)))
    }
}
