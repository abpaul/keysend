use std::{collections::HashMap, sync::Arc};

use axum::{
    extract::Query,
    http::StatusCode,
    response::{IntoResponse, Redirect},
    Extension,
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use loco_rs::prelude::*;
use serde::Deserialize;

use crate::{
    auth::{Authenticated, OAUTH_COOKIE, SESSION_COOKIE},
    content::Catalog,
    google, identity,
    language::Language,
    settings::Settings,
    workspace::WorkspaceStore,
};

fn sign_up_source(
    query: &HashMap<String, String>,
    catalog: &Catalog,
) -> Option<identity::SignUpSource> {
    query
        .get("from")
        .and_then(|value| identity::SignUpSource::parse(value))
        .filter(|source| {
            !source.is_resource()
                || catalog
                    .visible(crate::settings::wall_clock())
                    .iter()
                    .any(|entry| identity::SignUpSource::resource(&entry.path) == *source)
        })
}

async fn sign_in(
    ViewEngine(v): ViewEngine<TeraView>,
    Extension(catalog): Extension<Arc<Catalog>>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Response> {
    let source = sign_up_source(&query, &catalog);
    let from = source.as_ref().map(identity::SignUpSource::as_str);
    format::render().view(&v, "auth/sign_in.html", data!({"from": from}))
}

async fn start(
    State(ctx): State<AppContext>,
    Extension(settings): Extension<Arc<Settings>>,
    Extension(catalog): Extension<Arc<Catalog>>,
    jar: CookieJar,
    Query(query): Query<HashMap<String, String>>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    start_flow(
        &ctx,
        &settings,
        jar,
        &v,
        None,
        sign_up_source(&query, &catalog),
    )
    .await
}

async fn connect(
    Authenticated(signed_in): Authenticated,
    State(ctx): State<AppContext>,
    Extension(settings): Extension<Arc<Settings>>,
    jar: CookieJar,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    start_flow(&ctx, &settings, jar, &v, Some(&signed_in), None).await
}

async fn start_flow(
    ctx: &AppContext,
    settings: &Settings,
    jar: CookieJar,
    v: &TeraView,
    signed_in: Option<&identity::SignedIn>,
    source: Option<identity::SignUpSource>,
) -> Result<Response> {
    let text = signed_in
        .map_or(Language::En, |signed| signed.user.language)
        .text()?;
    let Some((client_id, _)) = settings.credentials() else {
        return error_page(
            v,
            StatusCode::SERVICE_UNAVAILABLE,
            text.written.page_signin_not_configured,
        );
    };
    if signed_in.is_some() && settings.master_key.is_none() {
        return error_page(
            v,
            StatusCode::SERVICE_UNAVAILABLE,
            text.written.page_google_connections_not_configured,
        );
    }
    let browser = jar
        .get(OAUTH_COOKIE)
        .map_or_else(identity::random_token, |cookie| cookie.value().to_owned());
    let (state, verifier) = identity::start_oauth(&ctx.db, &browser, signed_in, source).await?;
    let mut url = reqwest::Url::parse(&settings.google.authorization_url)
        .map_err(|_| Error::Message("Invalid Google authorization URL".into()))?;
    url.query_pairs_mut().extend_pairs([
        ("client_id", client_id),
        ("redirect_uri", &settings.redirect_uri),
        ("response_type", "code"),
        (
            "scope",
            if signed_in.is_some() {
                "openid email https://www.googleapis.com/auth/adwords"
            } else {
                "openid email profile"
            },
        ),
        ("state", &state),
        ("code_challenge", &identity::hash_token(&verifier)),
        ("code_challenge_method", "S256"),
    ]);
    if signed_in.is_some() {
        url.query_pairs_mut()
            .extend_pairs([("access_type", "offline"), ("prompt", "consent")]);
    }
    Ok((
        jar.add(cookie(
            OAUTH_COOKIE,
            browser,
            identity::STATE_SECONDS,
            settings.secure,
        )),
        Redirect::to(url.as_str()),
    )
        .into_response())
}

#[derive(Deserialize)]
struct Callback {
    state: Option<String>,
    code: Option<String>,
    error: Option<String>,
}

async fn callback(
    State(ctx): State<AppContext>,
    Extension(settings): Extension<Arc<Settings>>,
    jar: CookieJar,
    Query(query): Query<Callback>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    let text = Language::En.text()?;
    let cleared = jar
        .clone()
        .add(cookie(OAUTH_COOKIE, String::new(), 0, settings.secure));
    let Some(state) = query.state.as_deref() else {
        return Ok((
            cleared,
            error_page(
                &v,
                StatusCode::BAD_REQUEST,
                text.written.page_signin_state_invalid,
            )?,
        )
            .into_response());
    };
    let Some(browser) = jar.get(OAUTH_COOKIE) else {
        return Ok((
            cleared,
            error_page(
                &v,
                StatusCode::BAD_REQUEST,
                text.written.page_signin_state_invalid,
            )?,
        )
            .into_response());
    };
    let Some(flow) = identity::consume_oauth(&ctx.db, state, browser.value()).await? else {
        return Ok((
            cleared,
            error_page(
                &v,
                StatusCode::BAD_REQUEST,
                text.written.page_signin_state_invalid,
            )?,
        )
            .into_response());
    };
    if query.error.is_some() || query.code.as_deref().is_none_or(str::is_empty) {
        return Ok((
            cleared,
            error_page(
                &v,
                StatusCode::BAD_REQUEST,
                text.written.page_consent_incomplete,
            )?,
        )
            .into_response());
    }
    let signed_in = if let Some(workspace_id) = flow.workspace_id.as_deref() {
        let value = match jar.get(SESSION_COOKIE) {
            Some(token) => identity::authenticate(&ctx.db, token.value()).await?,
            None => None,
        };
        match value {
            Some(value)
                if value.workspace.id() == workspace_id
                    && flow.session_hash.as_deref() == Some(value.session_hash.as_str()) =>
            {
                Some(value)
            }
            _ => {
                return Ok((
                    cleared,
                    error_page(
                        &v,
                        StatusCode::FORBIDDEN,
                        text.written.page_connection_other_signin,
                    )?,
                )
                    .into_response())
            }
        }
    } else {
        None
    };
    let result = finish_callback(
        &ctx,
        &settings,
        query.code.as_deref().unwrap_or_default(),
        &flow.pkce_verifier,
        signed_in.as_ref(),
        flow.sign_up_source,
        &jar,
    )
    .await;
    match result {
        Ok((session_token, destination)) => {
            let jar = match session_token {
                Some(token) => cleared.add(cookie(
                    SESSION_COOKIE,
                    token,
                    identity::SESSION_SECONDS,
                    settings.secure,
                )),
                None => cleared,
            };
            Ok((jar, Redirect::to(&destination)).into_response())
        }
        Err(error) => {
            tracing::warn!(operation = "oauth_callback", %error, "Google callback failed");
            Ok((
                cleared,
                error_page(&v, StatusCode::BAD_GATEWAY, text.written.page_google_failed)?,
            )
                .into_response())
        }
    }
}

async fn finish_callback(
    ctx: &AppContext,
    settings: &Settings,
    code: &str,
    verifier: &str,
    signed_in: Option<&identity::SignedIn>,
    source: Option<identity::SignUpSource>,
    jar: &CookieJar,
) -> Result<(Option<String>, String)> {
    let tokens = google::exchange(settings, code, verifier).await?;
    let user = google::userinfo(settings, &tokens.access_token).await?;
    if let Some(signed_in) = signed_in {
        let key = settings
            .master_key
            .as_ref()
            .ok_or_else(|| Error::Message("Google Connections are not configured".into()))?;
        let refresh = tokens
            .refresh_token
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| Error::Message("Google did not return a refresh token".into()))?;
        let store = WorkspaceStore::new(&ctx.db, &signed_in.workspace);
        let connection = store
            .save_google_connection(&user.sub, &user.email, refresh, key)
            .await?;
        store
            .clear_history_error(&connection.id, crate::google_ads::REFUSED)
            .await?;
        let choices =
            google::discover_for_workspace(settings, &tokens.access_token, &store).await?;
        let selection = store.create_selection(&connection.id, choices).await?;
        Ok((None, format!("/ad-accounts/choose/{selection}")))
    } else {
        let token = identity::sign_in(
            &ctx.db,
            &user.sub,
            &user.email,
            &user.name,
            settings.now().timestamp(),
            source,
        )
        .await?;
        if let Some(old) = jar.get(SESSION_COOKIE) {
            identity::sign_out(&ctx.db, old.value()).await?;
        }
        let has_connection = match identity::authenticate(&ctx.db, &token).await? {
            Some(signed_in) => !WorkspaceStore::new(&ctx.db, &signed_in.workspace)
                .google_connections()
                .await?
                .is_empty(),
            None => false,
        };
        let destination = if has_connection {
            "/ad-accounts"
        } else {
            "/connect"
        };
        Ok((Some(token), destination.into()))
    }
}

async fn sign_out(
    State(ctx): State<AppContext>,
    Extension(settings): Extension<Arc<Settings>>,
    jar: CookieJar,
) -> Result<Response> {
    if let Some(token) = jar.get(SESSION_COOKIE) {
        identity::sign_out(&ctx.db, token.value()).await?;
    }
    Ok((
        jar.add(cookie(SESSION_COOKIE, String::new(), 0, settings.secure)),
        Redirect::to("/sign-in"),
    )
        .into_response())
}

fn cookie(name: &'static str, value: String, seconds: i64, secure: bool) -> Cookie<'static> {
    Cookie::build((name, value))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(secure)
        .max_age(
            std::time::Duration::from_secs(seconds as u64)
                .try_into()
                .unwrap(),
        )
        .build()
}

fn error_page(v: &TeraView, status: StatusCode, message: &str) -> Result<Response> {
    let mut response = format::render().view(v, "auth/error.html", data!({"message": message}))?;
    *response.status_mut() = status;
    Ok(response)
}

pub fn routes() -> Routes {
    Routes::new()
        .add("/sign-in", get(sign_in))
        .add("/auth/google/start", get(start))
        .add(
            "/auth/google/callback",
            get(callback).layer(axum::middleware::from_fn(
                crate::security::restore_oauth_query,
            )),
        )
        .add("/connections/google/start", post(connect))
        .add("/sign-out", post(sign_out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_visible_resource_can_supply_a_sign_up_source() {
        let catalog: Catalog = serde_json::from_value(serde_json::json!({
            "schema_version": 2,
            "calculator_js": "",
            "entries": [
                {"path": "/guides/example-resource", "title": "Example", "seo_title": "Example", "updated_at": null, "intent": "commercial", "related": [], "cta": {"heading": "", "text": "", "link": null}, "description": "Example", "publish_at": "2000-01-01T00:00:00Z", "kind": "guide", "cluster": "reporting-basics", "body_html": ""},
                {"path": "/guides/future-resource", "title": "Future", "seo_title": "Example", "updated_at": null, "intent": "commercial", "related": [], "cta": {"heading": "", "text": "", "link": null}, "description": "Future", "publish_at": "9999-01-01T00:00:00Z", "kind": "guide", "cluster": "reporting-basics", "body_html": ""}
            ]
        })).unwrap();
        for (path, allowed) in [
            ("/guides/example-resource", true),
            ("/guides/future-resource", false),
            ("/guides/unknown-resource", false),
        ] {
            let source = identity::SignUpSource::resource(path);
            let query = HashMap::from([("from".into(), source.as_str().into())]);
            assert_eq!(sign_up_source(&query, &catalog), allowed.then_some(source));
        }
        for value in ["resource", "home-hero", "home-hero-ro"] {
            let query = HashMap::from([("from".into(), value.into())]);
            assert_eq!(sign_up_source(&query, &catalog).unwrap().as_str(), value);
        }
    }
}
