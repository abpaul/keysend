use std::collections::HashSet;

use base64::Engine;
use sha2::{Digest, Sha256};

use axum::{
    extract::{Request, State},
    http::{header, HeaderValue, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

#[derive(Clone)]
pub struct Policy {
    pub origin: String,
    pub public_pages: HashSet<String>,
    pub release: Option<axum::http::HeaderValue>,
    pub content: std::sync::Arc<crate::content::Catalog>,
    app_csp: HeaderValue,
    content_csp: HeaderValue,
    hsts: Option<HeaderValue>,
}

const ANALYTICS: &str = "https://postdeploy.dev https://*.postdeploy.dev";

/// Turbo adds the style of its progress bar to the page. This is the hash of that style in the
/// vendored `turbo.min.js`; a test fails when a new Turbo changes it.
pub const TURBO_PROGRESS_BAR_STYLE: &str = "'sha256-WAyOw4V+FqDc35lQPyRADLBWbuNK8ahvYEaQIYF1+Ps='";

const FORM_TARGETS: &str =
    "'self' https://accounts.google.com https://checkout.stripe.com https://billing.stripe.com";

impl Policy {
    pub fn new(
        origin: String,
        public_pages: HashSet<String>,
        release: Option<HeaderValue>,
        content: std::sync::Arc<crate::content::Catalog>,
    ) -> Self {
        let directives = |script: &str, style_attributes: &str| {
            format!(
                "default-src 'self'; script-src 'self' {script}https://postdeploy.dev; style-src 'self' {TURBO_PROGRESS_BAR_STYLE}{style_attributes}; img-src 'self' data:; font-src 'self'; connect-src 'self' {ANALYTICS}; object-src 'none'; base-uri 'none'; form-action {FORM_TARGETS}; frame-ancestors 'none'"
            )
            .parse()
            .expect("valid Content-Security-Policy")
        };
        let calculator = (!content.calculator_js.is_empty()).then(|| {
            format!(
                "'sha256-{}' ",
                base64::engine::general_purpose::STANDARD
                    .encode(Sha256::digest(content.calculator_js.as_bytes()))
            )
        });
        Self {
            app_csp: directives("", ""),
            content_csp: directives(
                calculator.as_deref().unwrap_or_default(),
                "; style-src-attr 'unsafe-inline'",
            ),
            hsts: origin
                .starts_with("https://")
                .then(|| HeaderValue::from_static("max-age=31536000")),
            origin,
            public_pages,
            release,
            content,
        }
    }
}

pub async fn protect(State(policy): State<Policy>, mut request: Request, next: Next) -> Response {
    if !matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    ) && request.uri().path() != "/stripe/webhook"
        && !same_origin(&request, &policy.origin)
    {
        return (StatusCode::FORBIDDEN, [(header::CACHE_CONTROL, "no-store")]).into_response();
    }
    if request.uri().path() == "/auth/google/callback" && request.uri().query().is_some() {
        let uri = request.uri().clone();
        request.extensions_mut().insert(OAuthCallbackUri(uri));
        *request.uri_mut() = axum::http::Uri::from_static("/auth/google/callback");
    }
    let private = request.headers().contains_key(header::AUTHORIZATION)
        || request
            .headers()
            .get_all(header::COOKIE)
            .iter()
            .any(|value| {
                value.to_str().map_or(true, |cookies| {
                    cookies.contains(crate::auth::SESSION_COOKIE) || cookies.contains('%')
                })
            })
        || !matches!(*request.method(), Method::GET | Method::HEAD);
    let cache_control = if private && !crate::assets::is_fingerprinted(request.uri().path()) {
        Some("no-store")
    } else {
        match request.uri().path() {
            path if (policy.public_pages.contains(path) || crate::content::public_path(path))
                && request.uri().query().is_none() =>
            {
                Some("public, max-age=0, s-maxage=300, must-revalidate")
            }
            path if crate::assets::is_fingerprinted(path) => {
                Some("public, max-age=31536000, immutable")
            }
            "/sample-report.pdf" if request.uri().query().is_none() => {
                Some("public, max-age=0, must-revalidate")
            }
            path if path.starts_with("/static/") => Some("no-cache"),
            _ => Some("no-store"),
        }
    };
    let public_html = cache_control.is_some_and(|value| value.contains("s-maxage"));
    let content_page = crate::content::html_page(request.uri().path());
    let rendered_at = crate::settings::wall_clock();
    let mut response = next.run(request).await;
    let unsafe_response = response
        .headers()
        .get(header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("no-store") || value.contains("private"))
        || (public_html
            && (response.status() != StatusCode::OK
                || !response
                    .headers()
                    .get(header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .is_some_and(|value| {
                        value.starts_with("text/html")
                            || value.starts_with("application/xml")
                            || value.starts_with("application/rss+xml")
                            || value.starts_with("text/plain")
                            || value.starts_with("text/markdown")
                    })));
    let cache_control = if (response.status().is_success()
        || response.status() == StatusCode::NOT_MODIFIED)
        && !response.headers().contains_key(header::SET_COOKIE)
        && !unsafe_response
    {
        cache_control
    } else {
        Some("no-store")
    };
    if let Some(value) = cache_control {
        let value = if public_html && value.contains("s-maxage") {
            format!(
                "public, max-age=0, s-maxage={}, must-revalidate",
                policy
                    .content
                    .cache_seconds(rendered_at, crate::settings::wall_clock())
            )
        } else {
            value.to_owned()
        };
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, value.parse().unwrap());
    }
    if let Some(release) = policy.release {
        response.headers_mut().insert("x-keysend-release", release);
    }
    let headers = response.headers_mut();
    headers.insert(header::REFERRER_POLICY, "same-origin".parse().unwrap());
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        if content_page {
            policy.content_csp
        } else {
            policy.app_csp
        },
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    if let Some(hsts) = policy.hsts {
        headers.insert(header::STRICT_TRANSPORT_SECURITY, hsts);
    }
    response
}

fn same_origin(request: &Request, expected: &str) -> bool {
    if let Some(site) = request.headers().get("sec-fetch-site") {
        return site == "same-origin";
    }
    request
        .headers()
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|origin| origin == expected)
}

#[derive(Clone)]
struct OAuthCallbackUri(axum::http::Uri);

pub async fn restore_oauth_query(mut request: Request, next: Next) -> Response {
    if let Some(OAuthCallbackUri(uri)) = request.extensions_mut().remove::<OAuthCallbackUri>() {
        *request.uri_mut() = uri;
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rendering_across_publication_does_not_cache_the_older_index() {
        let catalog = serde_json::from_value(serde_json::json!({
            "schema_version": 2,
            "calculator_js": "",
            "entries": [{
                "path": "/blog/scheduled-article",
                "title": "Scheduled article",
                "seo_title": "Scheduled article",
                "description": "Scheduled article",
                "updated_at": null,
                "intent": "informational",
                "related": [],
                "cta": {"heading": "", "text": "", "link": null},
                "publish_at": crate::settings::wall_clock() + chrono::Duration::milliseconds(300),
                "kind": "article",
                "cluster": "reporting",
                "body_html": "<p>Scheduled article</p>"
            }]
        }))
        .unwrap();
        let policy = Policy::new(
            "http://localhost".into(),
            HashSet::from(["/".into()]),
            None,
            std::sync::Arc::new(catalog),
        );
        let router = axum::Router::new()
            .route(
                "/",
                axum::routing::get(|| async {
                    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
                    axum::response::Html("Index rendered before publication")
                }),
            )
            .layer(axum::middleware::from_fn_with_state(policy, protect));
        let server = axum_test::TestServer::new(router).unwrap();
        let response = server.get("/").await;
        assert_eq!(response.status_code(), StatusCode::OK);
        assert_eq!(response.text(), "Index rendered before publication");
        assert_eq!(
            response.header(header::CACHE_CONTROL),
            "public, max-age=0, s-maxage=0, must-revalidate"
        );
    }

    #[tokio::test]
    async fn a_session_keeps_fingerprinted_assets_immutable_and_other_assets_private() {
        let catalog = serde_json::from_value(serde_json::json!({
            "schema_version": 2,
            "calculator_js": "",
            "entries": []
        }))
        .unwrap();
        let policy = Policy::new(
            "http://localhost".into(),
            HashSet::new(),
            None,
            std::sync::Arc::new(catalog),
        );
        let router = axum::Router::new()
            .route("/static/{*path}", axum::routing::get(|| async { "asset" }))
            .layer(axum::middleware::from_fn_with_state(policy, protect));
        let server = axum_test::TestServer::new(router).unwrap();
        let cookie = format!("{}=token", crate::auth::SESSION_COOKIE);
        let fingerprinted = format!("/static/{}/app.css", "a".repeat(64));
        for (path, expected) in [
            (
                fingerprinted.as_str(),
                "public, max-age=31536000, immutable",
            ),
            ("/static/app.css", "no-store"),
        ] {
            let response = server.get(path).add_header(header::COOKIE, &cookie).await;
            assert_eq!(response.status_code(), StatusCode::OK);
            assert_eq!(response.header(header::CACHE_CONTROL), expected);
        }
    }
}
