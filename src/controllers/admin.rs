use crate::{
    identity::{self, admin},
    language::Language,
    settings::Settings,
};
use axum::{
    extract::{Form, FromRequestParts, OptionalFromRequestParts},
    http::{request::Parts, HeaderMap, StatusCode},
    response::{IntoResponse, Redirect},
    Extension,
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use loco_rs::prelude::*;
use serde::Deserialize;
use std::sync::Arc;

pub const SESSION_COOKIE: &str = "ks_admin";
pub const CHALLENGE_COOKIE: &str = "ks_admin_challenge";

pub struct Admin {
    pub email: String,
}
impl FromRequestParts<AppContext> for Admin {
    type Rejection = Response;
    async fn from_request_parts(
        parts: &mut Parts,
        ctx: &AppContext,
    ) -> std::result::Result<Self, Self::Rejection> {
        <Self as OptionalFromRequestParts<AppContext>>::from_request_parts(parts, ctx)
            .await?
            .ok_or_else(|| Redirect::to("/admin").into_response())
    }
}

impl OptionalFromRequestParts<AppContext> for Admin {
    type Rejection = Response;
    async fn from_request_parts(
        parts: &mut Parts,
        ctx: &AppContext,
    ) -> std::result::Result<Option<Self>, Self::Rejection> {
        crate::language::set_current(Language::En)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?;
        let jar = CookieJar::from_request_parts(parts, ctx)
            .await
            .map_err(IntoResponse::into_response)?;
        let Some(cookie) = jar.get(SESSION_COOKIE) else {
            return Ok(None);
        };
        let settings = parts
            .extensions
            .get::<Arc<Settings>>()
            .cloned()
            .ok_or_else(|| StatusCode::INTERNAL_SERVER_ERROR.into_response())?;
        let email = admin::authenticate(&ctx.db, &settings, cookie.value())
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?;
        Ok(email.map(|email| Self { email }))
    }
}

fn cookie(name: &'static str, value: String, secure: bool, seconds: i64) -> Cookie<'static> {
    Cookie::build((name, value))
        .path("/admin")
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Strict)
        .max_age(
            std::time::Duration::from_secs(seconds as u64)
                .try_into()
                .unwrap(),
        )
        .build()
}
fn ip(headers: &HeaderMap) -> &str {
    headers
        .get("cf-connecting-ip")
        .and_then(|v| v.to_str().ok())
        .or_else(|| {
            headers
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.split(',').next())
        })
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or("unknown")
}
mod pages;
async fn index(
    admin: Option<Admin>,
    settings: Extension<Arc<Settings>>,
    ctx: State<AppContext>,
    v: ViewEngine<TeraView>,
) -> Result<Response> {
    if let Some(admin) = admin {
        pages::overview(admin, settings, ctx, v).await
    } else {
        format::render().view(&v.0, "admin/sign_in.html", data!({}))
    }
}
#[derive(Deserialize)]
struct EmailForm {
    email: String,
}
async fn request_code(
    Extension(settings): Extension<Arc<Settings>>,
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    jar: CookieJar,
    Form(form): Form<EmailForm>,
) -> Result<Response> {
    let secure = settings.secure;
    let email = form.email.trim().to_lowercase();
    let token = identity::random_token();
    if admin::allowed(&settings, &email) {
        tracing::info!(event = "admin_code_requested", email);
    } else {
        tracing::info!(event = "admin_code_requested");
    }
    if let Some(code) = admin::challenge(&ctx.db, &settings, &email, ip(&headers), &token).await? {
        tokio::spawn(async move {
            let text = format!("Your KeySend admin code is {code}. It expires in 10 minutes.");
            let html = format!("<p>{text}</p>");
            if crate::monitoring::email::send(
                &settings,
                &email,
                "Your KeySend admin code",
                &text,
                Some(&html),
            )
            .await
            .is_err()
            {
                tracing::warn!(event = "admin_code_not_sent", email);
            }
        });
    }
    Ok((
        jar.add(cookie(
            CHALLENGE_COOKIE,
            token,
            secure,
            admin::CHALLENGE_SECONDS,
        )),
        Redirect::to("/admin/verify"),
    )
        .into_response())
}
async fn verify_page(ViewEngine(v): ViewEngine<TeraView>) -> Result<Response> {
    verification_page(&v, None)
}
fn verification_page(v: &TeraView, error: Option<&str>) -> Result<Response> {
    format::render().view(v, "admin/verify.html", data!({"error":error}))
}
#[derive(Deserialize)]
struct CodeForm {
    code: String,
}
async fn verify_code(
    Extension(settings): Extension<Arc<Settings>>,
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    jar: CookieJar,
    ViewEngine(v): ViewEngine<TeraView>,
    Form(form): Form<CodeForm>,
) -> Result<Response> {
    let token = jar.get(CHALLENGE_COOKIE).map_or("", Cookie::value);
    match admin::verify(&ctx.db, &settings, token, &form.code, ip(&headers)).await? {
        admin::Verification::SignedIn(token) => {
            if let Some(old) = jar.get(SESSION_COOKIE) {
                admin::sign_out(&ctx.db, old.value()).await?;
            }
            Ok((
                jar.add(cookie(CHALLENGE_COOKIE, String::new(), settings.secure, 0))
                    .add(cookie(
                        SESSION_COOKIE,
                        token,
                        settings.secure,
                        admin::SESSION_SECONDS,
                    )),
                Redirect::to("/admin"),
            )
                .into_response())
        }
        admin::Verification::Refused => verification_page(&v, Some("admin_code_refused")),
    }
}
async fn sign_out(
    Extension(settings): Extension<Arc<Settings>>,
    State(ctx): State<AppContext>,
    jar: CookieJar,
) -> Result<Response> {
    if let Some(old) = jar.get(SESSION_COOKIE) {
        admin::sign_out(&ctx.db, old.value()).await?;
    }
    Ok((
        jar.add(cookie(SESSION_COOKIE, String::new(), settings.secure, 0)),
        Redirect::to("/admin"),
    )
        .into_response())
}
pub fn routes() -> Routes {
    Routes::new()
        .add("/admin", get(index))
        .add("/admin/customers", get(pages::customers))
        .add("/admin/customers/{workspace_id}", get(pages::customer))
        .add("/admin/revenue", get(pages::revenue))
        .add("/admin/usage", get(pages::usage))
        .add("/admin/operations", get(pages::operations))
        .add("/admin/code", post(request_code))
        .add("/admin/verify", get(verify_page).post(verify_code))
        .add("/admin/sign-out", post(sign_out))
}
