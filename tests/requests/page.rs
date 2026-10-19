use keysend::app::App;
use loco_rs::testing::prelude::*;
use serial_test::serial;

use super::catalog::{assert_publicly_cached, Catalog};

const AUDIENCE_PATHS: [&str; 3] = [
    "/for/business-owners",
    "/for/agency-clients",
    "/for/marketing-managers",
];

fn attribute_values<'a>(body: &'a str, attribute: &str) -> Vec<&'a str> {
    body.split(&format!(" {attribute}=\""))
        .skip(1)
        .map(|rest| rest.split('"').next().unwrap())
        .collect()
}

fn structured_data(body: &str) -> Vec<serde_json::Value> {
    body.split("<script type=\"application/ld+json\">")
        .skip(1)
        .map(|rest| serde_json::from_str(rest.split("</script>").next().unwrap()).unwrap())
        .collect()
}

fn shown_questions(body: &str) -> Vec<(String, String)> {
    body.split("<div class=\"faq\">")
        .nth(1)
        .unwrap()
        .split("<summary>")
        .skip(1)
        .map(|rest| {
            let (question, rest) = rest.split_once("</summary>").unwrap();
            let answer = rest
                .split_once("<p>")
                .unwrap()
                .1
                .split_once("</p>")
                .unwrap()
                .0;
            (question.to_owned(), answer.to_owned())
        })
        .collect()
}

fn schema_questions(graph: &serde_json::Value) -> Vec<(String, String)> {
    graph["@graph"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["@type"] == "FAQPage")
        .unwrap()["mainEntity"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            assert_eq!(item["@type"], "Question");
            assert_eq!(item["acceptedAnswer"]["@type"], "Answer");
            (
                item["name"].as_str().unwrap().to_owned(),
                item["acceptedAnswer"]["text"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

#[tokio::test]
#[serial]
async fn the_home_page_leads_to_sign_in_states_the_price_and_shows_examples_without_controls() {
    request::<App, _, _>(|request, _ctx| async move {
        let res = request.get("/").await;
        assert_eq!(res.status_code(), 200, "{}", res.text());
        let body = res.text();

        for source in ["home-nav", "home-hero", "home-price", "home-closing"] {
            assert_eq!(
                body.matches(&format!("href=\"/sign-in?from={source}\""))
                    .count(),
                1,
                "{source}"
            );
        }
        assert_eq!(body.matches("href=\"/sign-in").count(), 5);
        let at = |text: &str| body.find(text).unwrap();
        assert!(at("from=home-hero") < at(" id=\"how\""));
        assert!(at(" id=\"price\"") < at("from=home-price"));
        assert!(at("from=home-price") < at(" id=\"questions\""));
        assert!(at(" id=\"start\"") < at("from=home-closing"));
        for audience in [
            "/for/business-owners",
            "/for/agency-clients",
            "/for/marketing-managers",
        ] {
            assert!(body.contains(&format!("href=\"{audience}\"")), "{audience}");
        }

        assert!(body.contains("14 days"));
        assert!(body.contains("href=\"/terms#refunds\""));
        let hero = body.split("id=\"how\"").next().unwrap();
        assert!(hero.contains("$49"));
        assert!(hero.contains("plus applicable tax"));
        assert!(hero.contains("up to 5 Ad Accounts"));
        assert!(body.contains("$49"));
        assert!(body.contains("per month, plus applicable tax, for up to 5 Ad Accounts."));
        assert!(body.contains("\"price\":\"49\",\"priceCurrency\":\"USD\""));
        assert!(body.contains("<link rel=\"canonical\" href=\"https://keysend.app/\">"));
        assert!(body.contains("<meta property=\"og:url\" content=\"https://keysend.app/\">"));
        assert!(!body.contains("noindex"));

        assert!(body.contains("$3,738"));
        assert!(body.contains("conversions rose from 95 to 102"));
        assert!(body.contains("class=\"campaign-name\">Search - Emergency</"));
        for gone in [
            "<form",
            "<button",
            "/ad-accounts",
            "window-switch.js",
            "data-swap",
            " style=\"",
            "<a class=\"campaign-name\"",
            "invented dental practice",
            "A real KeySend",
            "Not useful",
            ">Done<",
            "Code calculates",
            "AI writes",
        ] {
            assert!(!body.contains(gone), "{gone} is in the home page");
        }

        let ids = attribute_values(&body, "id");
        for id in &ids {
            assert_eq!(ids.iter().filter(|other| *other == id).count(), 1, "{id}");
        }
        for label in attribute_values(&body, "aria-labelledby") {
            assert!(ids.contains(&label), "aria-labelledby names {label}");
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn scheduled_resources_hide_future_pages_and_keep_private_requests_uncacheable() {
    request::<App, _, _>(|request, _ctx| async move {
        let catalog = Catalog::load();
        let future = catalog.scheduled();
        for path in [
            "/blog",
            "/guides",
            "/tools",
            "/resources",
            "/sitemap.xml",
            "/feed.xml",
        ] {
            let response = request.get(path).await;
            assert_eq!(response.status_code(), 200, "{path}");
            let body = response.text();
            for entry in &future {
                assert!(!body.contains(&entry.path), "future path in {path}");
                assert!(!body.contains(&entry.title), "future title in {path}");
            }
            let private = request
                .get(path)
                .add_header("Authorization", "Bearer cache-test")
                .await;
            assert_eq!(private.header("cache-control"), "no-store", "{path}");
        }
        for entry in future {
            let response = request.get(&entry.path).await;
            assert_eq!(response.status_code(), 404);
            assert_eq!(response.header("cache-control"), "no-store");
        }
        for path in [
            "/content/seo/catalog.json",
            "/static/content/seo/catalog.json",
            "/blog/not-a-published-article",
        ] {
            let response = request.get(path).await;
            assert_eq!(response.status_code(), 404);
            assert_eq!(response.header("cache-control"), "no-store");
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn landing_page_assets_are_served() {
    request::<App, _, _>(|request, _ctx| async move {
        let home = request.get("/").await.text();
        let mut paths: Vec<&str> = home
            .split('"')
            .filter(|part| part.starts_with("/static/"))
            .flat_map(|part| part.split(", "))
            .map(|part| part.split_whitespace().next().unwrap())
            .collect();
        paths.push("/static/fonts/plus-jakarta-sans.woff2");
        assert!(paths.len() >= 5, "found only {paths:?}");

        for path in paths {
            assert_eq!(request.get(path).await.status_code(), 200, "{path}");
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn readiness_reports_ok() {
    request::<App, _, _>(|request, _ctx| async move {
        let res = request.get("/_readiness").await;

        assert_eq!(res.status_code(), 200);
        assert_eq!(res.text(), r#"{"ok":true}"#);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn no_response_names_the_framework() {
    request::<App, _, _>(|request, _ctx| async move {
        for path in ["/", "/sign-in", "/_readiness", "/missing"] {
            let response = request.get(path).await;
            assert!(
                response.maybe_header("x-powered-by").is_none(),
                "{path} sends X-Powered-By"
            );
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn unknown_path_is_not_found() {
    request::<App, _, _>(|request, _ctx| async move {
        let res = request.get("/no-such-page").await;

        assert_eq!(res.status_code(), 404);
        assert!(res.text().contains("This page does not exist"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn missing_static_file_is_not_found() {
    request::<App, _, _>(|request, _ctx| async move {
        let res = request.get("/static/no-such-file.css").await;

        assert_eq!(res.status_code(), 404);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn sign_in_offers_google() {
    request::<App, _, _>(|request, _ctx| async move {
        let res = request.get("/sign-in").await;

        assert_eq!(res.status_code(), 200);
        assert!(res.text().contains("Sign in with Google"));
        assert!(res.text().contains("href=\"/auth/google/start\""));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn static_files_are_revalidated_so_a_new_release_replaces_them() {
    request::<App, _, _>(|request, _ctx| async move {
        let res = request.get("/static/styles.css").await;

        assert_eq!(res.status_code(), 200);
        assert_eq!(res.header("cache-control"), "no-cache");
        let modified = res.header("last-modified");
        let unchanged = request
            .get("/static/styles.css")
            .add_header("if-modified-since", modified)
            .await;
        assert_eq!(unchanged.status_code(), 304);
        assert_eq!(unchanged.header("cache-control"), "no-cache");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_audience_pages_of_the_old_product_are_gone() {
    request::<App, _, _>(|request, _ctx| async move {
        for slug in [
            "consultants",
            "agencies",
            "freelancers",
            "marketing-teams",
            "xyz",
        ] {
            let res = request.get(&format!("/for/{slug}")).await;

            assert_eq!(res.status_code(), 404, "{slug}");
            assert!(res.text().contains("This page does not exist"));
        }
        let sitemap = request.get("/sitemap.xml").await.text();
        assert_eq!(sitemap.matches("/for/").count(), 3);
        for path in AUDIENCE_PATHS {
            assert!(
                sitemap.contains(&format!("<url><loc>https://keysend.app{path}</loc></url>")),
                "{path}"
            );
        }
        for path in ["/", "/privacy", "/blog", "/for/business-owners"] {
            let body = request.get(path).await.text();
            assert!(!body.contains("/for/freelancers"), "{path}");
            for audience in AUDIENCE_PATHS {
                assert!(
                    body.contains(&format!("href=\"{audience}\"")),
                    "{path} {audience}"
                );
            }
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn resource_pages_show_what_keysend_is_built_with() {
    request::<App, _, _>(|request, _ctx| async move {
        for path in ["/resources"] {
            let body = request.get(path).await.text();
            assert!(body.contains("Built with"), "{path}");
            assert!(body.contains(r#"href="https://postdeploy.dev/""#), "{path}");
            for logo in ["rust", "loco", "postdeploy", "ovhcloud"] {
                let suffix = format!("/images/built-with/{logo}.svg");
                let file = body
                    .split('"')
                    .find(|part| part.starts_with("/static/") && part.ends_with(&suffix))
                    .unwrap();
                assert_eq!(request.get(file).await.status_code(), 200, "{file}");
            }
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn privacy_page_states_how_google_data_is_used() {
    request::<App, _, _>(|request, _ctx| async move {
        let res = request.get("/privacy").await;

        assert_eq!(res.status_code(), 200);
        let body = res.text();
        assert!(body.contains("<title>Privacy | KeySend</title>"));
        assert!(body.contains("Google API Services User Data Policy"));
        assert!(body.contains("Limited Use"));
        assert!(body.contains("hello@keysend.app"));
        assert!(body.contains("https://myaccount.google.com/connections"));
        assert!(!body.contains("cannot change"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn every_public_page_links_to_the_security_page_and_it_names_its_page_and_contacts() {
    request::<App, _, _>(|request, _ctx| async move {
        for path in [
            "/",
            "/privacy",
            "/terms",
            "/for/agency-clients",
            "/resources",
        ] {
            let body = request.get(path).await.text();
            assert!(body.contains(r#"href="/security""#), "{path}");
        }

        let response = request.get("/security").await;
        assert_eq!(response.status_code(), 200);
        let body = response.text();
        assert!(body.contains("<title>Security and your data | KeySend</title>"));
        assert!(body.contains(r#"<link rel="canonical" href="https://keysend.app/security">"#));
        for href in [
            "/privacy#where",
            "/settings",
            "mailto:hello@keysend.app?subject=Privacy",
        ] {
            assert!(body.contains(&format!(r#"href="{href}""#)), "{href}");
        }
        let graph = &structured_data(&body)[0]["@graph"];
        let node = |kind: &str| {
            graph
                .as_array()
                .unwrap()
                .iter()
                .find(|node| node["@type"] == kind)
                .unwrap()
                .clone()
        };
        assert_eq!(
            node("Organization")["@id"],
            "https://keysend.app/#organization"
        );
        let page = node("WebPage");
        assert_eq!(page["url"], "https://keysend.app/security");
        assert_eq!(page["name"], "Security and your data");
        assert_eq!(
            page["publisher"]["@id"],
            "https://keysend.app/#organization"
        );
        let crumbs = node("BreadcrumbList")["itemListElement"].clone();
        assert_eq!(crumbs[0]["item"], "https://keysend.app/");
        assert_eq!(crumbs[1]["name"], "Security");
        assert_eq!(crumbs[1]["item"], "https://keysend.app/security");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn home_and_sign_in_link_to_the_privacy_page() {
    request::<App, _, _>(|request, _ctx| async move {
        for path in ["/", "/sign-in"] {
            let body = request.get(path).await.text();
            assert!(body.contains("href=\"/privacy\""), "{path}");
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn public_pages_load_the_visit_counter_and_signed_in_pages_do_not() {
    request::<App, _, _>(|request, _ctx| async move {
        for path in ["/", "/privacy", "/resources", "/sign-in"] {
            let body = request.get(path).await.text();
            assert_eq!(
                body.matches("https://postdeploy.dev/m.js?token=msr_")
                    .count(),
                1,
                "{path}"
            );
        }
        let error = request.get("/auth/google/callback").await.text();
        assert!(!error.contains("postdeploy.dev/m.js"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn hashed_assets_are_immutable_for_everyone_and_other_private_requests_are_not_cached() {
    request::<App, _, _>(|request, ctx| async move {
        let home = request.get("/").await.text();
        let css_path = home.split('"').find(|part| part.starts_with("/static/") && part.ends_with("/styles.css")).unwrap();
        let css = request.get(css_path).await;
        assert_eq!(css.status_code(), 200);
        assert_eq!(css.header("cache-control"), "public, max-age=31536000, immutable");
        let font_path = css.text().split('"').find(|part| part.starts_with("/static/") && part.ends_with(".woff2")).unwrap().to_owned();
        assert_eq!(request.get(&font_path).await.status_code(), 200);
        assert_eq!(request.get("/static/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/missing.css").await.header("cache-control"), "no-store");
        for name in ["Cache proof Alice", "Cache proof Bob"] {
            let token = keysend::identity::sign_in(&ctx.db, name, &format!("{}@example.invalid", name.replace(' ', "-")), name, chrono::Utc::now().timestamp(), None).await.unwrap();
            let cookie = axum_extra::extract::cookie::Cookie::new(keysend::auth::SESSION_COOKIE, token);
            for path in ["/", "/privacy", "/sign-in", "/accounts", "/ad-accounts", "/subscription", "/billing"] {
                let response = request.get(path).add_cookie(cookie.clone()).await;
                assert_eq!(response.header("cache-control"), "no-store", "{path}");
            }
            let css = request.get(css_path).add_cookie(cookie.clone()).await;
            assert_eq!(css.header("cache-control"), "public, max-age=31536000, immutable");
            assert!(css.maybe_header("set-cookie").is_none());
            let email = format!("{}@example.invalid", name.replace(' ', "-"));
            let other = if name.ends_with("Alice") { "Cache-proof-Bob" } else { "Cache-proof-Alice" };
            let settings = request.get("/settings").add_cookie(cookie).await;
            assert_eq!(settings.status_code(), 200);
            assert!(settings.text().contains(&email));
            assert!(!settings.text().contains(other));
        }
        assert_eq!(request.get(css_path).add_header("authorization", "Bearer cache-proof").await.header("cache-control"), "public, max-age=31536000, immutable");
        assert_eq!(request.get("/").add_header("authorization", "Bearer cache-proof").await.header("cache-control"), "no-store");
        assert_eq!(request.get(css_path).await.header("cache-control"), "public, max-age=31536000, immutable");
    }).await;
}

#[tokio::test]
#[serial]
async fn only_allowlisted_anonymous_html_is_publicly_cacheable() {
    request::<App, _, _>(|request, _ctx| async move {
        let public: Vec<String> =
            serde_json::from_slice(&std::fs::read("config/cache-policy.json").unwrap()).unwrap();
        assert_eq!(public.len(), 15);
        for path in public {
            let response = request.get(&path).await;
            assert_eq!(response.status_code(), 200);
            assert_publicly_cached(response.header("cache-control").to_str().unwrap());
            assert!(response.maybe_header("set-cookie").is_none());
            assert_eq!(
                request
                    .get(&path)
                    .add_header("authorization", "")
                    .await
                    .header("cache-control"),
                "no-store"
            );
            assert_eq!(
                request
                    .get(&format!("{path}?probe=1"))
                    .await
                    .header("cache-control"),
                "no-store"
            );
        }
        for path in [
            "/sign-in",
            "/accounts",
            "/billing",
            "/subscription",
            "/stripe/webhook",
            "/connect",
            "/_readiness",
            "/for/unknown",
            "/auth/google/start",
        ] {
            assert_eq!(
                request.get(path).await.header("cache-control"),
                "no-store",
                "{path}"
            );
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn home_has_one_h1_and_no_repeated_element_id() {
    request::<App, _, _>(|request, _ctx| async move {
        let body = request.get("/").await.text();

        assert_eq!(body.matches("<h1").count(), 1);

        let ids: Vec<&str> = body
            .split(" id=\"")
            .skip(1)
            .filter_map(|rest| rest.split('"').next())
            .collect();
        assert!(ids.len() >= 6, "found {} ids", ids.len());
        let mut seen = std::collections::HashSet::new();
        for id in &ids {
            assert!(seen.insert(id), "id {id} appears twice");
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn terms_page_states_the_operator_the_prices_and_the_contact() {
    request::<App, _, _>(|request, _ctx| async move {
        let res = request.get("/terms").await;

        assert_eq!(res.status_code(), 200);
        let body = res.text();
        assert!(body.contains("<title>Terms | KeySend</title>"));
        assert!(body.contains(r#"<link rel="canonical" href="https://keysend.app/terms">"#));
        assert!(body.contains("ALPLUS TECHNOLOGIES S.R.L."));
        assert!(body.contains("$49 a month"));
        assert!(body.contains("Free, for 14 days"));
        for gone in ["$4.99", "$39.99", "slides", "PowerPoint", "20 Ad Accounts"] {
            assert!(!body.contains(gone), "{gone} is in the terms");
        }
        assert!(body.contains("hello@keysend.app"));
        assert!(body.contains(r#"href="/privacy""#));
        assert!(!body.contains("cannot change"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn home_and_sign_in_link_to_the_terms_page() {
    request::<App, _, _>(|request, _ctx| async move {
        for path in ["/", "/sign-in"] {
            let body = request.get(path).await.text();
            assert!(body.contains(r#"href="/terms""#), "{path}");
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn privacy_page_links_to_the_terms_and_keeps_the_google_policy_statement() {
    request::<App, _, _>(|request, _ctx| async move {
        let body = request.get("/privacy").await.text();

        assert!(body.contains(r#"<a href="/terms">terms</a>"#));
        assert!(body.contains("Google API Services User Data Policy"));
        assert!(body.contains("Limited Use"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn each_legal_page_has_one_h1_and_a_contents_link_for_every_section() {
    request::<App, _, _>(|request, _ctx| async move {
        for path in ["/privacy", "/terms", "/security"] {
            let body = request.get(path).await.text();
            assert_eq!(body.matches("<h1").count(), 1, "{path}");

            let targets: Vec<&str> = body
                .split(r##"<li><a href="#"##)
                .skip(1)
                .filter_map(|rest| rest.split('"').next())
                .collect();
            assert_eq!(
                targets.len(),
                body.matches("<section id=").count()
                    + body.matches(r#"<section class="legal-short" id="#).count(),
                "{path}"
            );
            for target in targets {
                assert_eq!(
                    body.matches(&format!(r#" id="{target}""#)).count(),
                    1,
                    "{path} #{target}"
                );
            }
        }
    })
    .await;
}

fn content_security_policy(response: &axum_test::TestResponse) -> String {
    response
        .header("content-security-policy")
        .to_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
#[serial]
async fn every_response_has_a_content_security_policy_without_inline_script_or_style() {
    request::<App, _, _>(|request, _ctx| async move {
        for path in [
            "/",
            "/sign-in",
            "/ad-accounts",
            "/privacy",
            "/missing-page",
            "/static/styles.css",
            "/sitemap.xml",
        ] {
            let response = request.get(path).await;
            let policy = content_security_policy(&response);
            let directives = policy.split("; ").collect::<Vec<_>>();
            for expected in [
                "default-src 'self'",
                "img-src 'self' data:",
                "font-src 'self'",
                "object-src 'none'",
                "base-uri 'none'",
                "frame-ancestors 'none'",
            ] {
                assert!(directives.contains(&expected), "{path}: {policy}");
            }
            assert!(
                directives
                    .iter()
                    .any(|directive| directive.starts_with("script-src 'self' ")),
                "{path}: {policy}"
            );
            assert!(
                directives
                    .iter()
                    .any(|directive| directive.starts_with("style-src 'self' 'sha256-")),
                "{path}: {policy}"
            );
            assert!(!policy.contains("unsafe-inline"), "{path}: {policy}");
            assert!(!policy.contains("unsafe-eval"), "{path}: {policy}");
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_calculator_page_allows_its_one_script_by_hash_and_inline_style_attributes_only_on_content_pages(
) {
    use base64::Engine;
    use sha2::Digest;
    request::<App, _, _>(|request, _ctx| async move {
        let catalog = Catalog::load();
        let script = catalog.calculator_js.as_str();
        assert!(!script.is_empty());
        let hash = base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(script));
        let tool = catalog.published_kind("tool")[0];
        let response = request.get(&tool.path).await;
        assert_eq!(response.status_code(), 200);
        assert!(response
            .text()
            .contains(&format!("<script>{script}</script>")));
        let policy = content_security_policy(&response);
        assert!(
            policy.contains(&format!("script-src 'self' 'sha256-{hash}' ")),
            "{policy}"
        );
        assert!(
            policy.contains("style-src-attr 'unsafe-inline'"),
            "{policy}"
        );
        let home = content_security_policy(&request.get("/").await);
        assert!(
            !home.contains(&format!("'sha256-{hash}'")) && !home.contains("style-src-attr"),
            "{home}"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn every_response_forbids_framing_and_content_type_sniffing() {
    request::<App, _, _>(|request, _ctx| async move {
        for path in [
            "/",
            "/sign-in",
            "/security",
            "/missing-page",
            "/static/styles.css",
        ] {
            let response = request.get(path).await;
            assert_eq!(response.header("x-frame-options"), "DENY", "{path}");
            assert_eq!(
                response.header("x-content-type-options"),
                "nosniff",
                "{path}"
            );
            assert!(
                response.maybe_header("strict-transport-security").is_none(),
                "{path}: an http origin sends no HSTS"
            );
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_policy_allows_the_progress_bar_style_of_the_vendored_turbo() {
    use base64::Engine;
    use sha2::Digest;
    let script = std::fs::read_to_string("assets/static/turbo.min.js").unwrap();
    let start = script.find("static get defaultCSS(){return h`").unwrap()
        + "static get defaultCSS(){return h`".len();
    let template = &script[start..start + script[start..].find("`}hiding=!1").unwrap()];
    let template = template
        .replace("${Re.animationDuration}", "300")
        .replace("${Re.animationDuration/2}", "150");
    let lines = template
        .strip_prefix('\n')
        .unwrap_or(&template)
        .split('\n')
        .collect::<Vec<_>>();
    let indent = lines[0].len() - lines[0].trim_start().len();
    let style = lines
        .iter()
        .map(|line| &line[indent.min(line.len())..])
        .collect::<Vec<_>>()
        .join("\n");
    let hash = base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(style));
    request::<App, _, _>(|request, _ctx| async move {
        let policy = content_security_policy(&request.get("/sign-in").await);
        assert!(
            policy.contains(&format!("style-src 'self' 'sha256-{hash}'")),
            "{policy}"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_consultant_opens_the_sample_report_from_the_hero_and_reads_the_limit_and_the_contact_for_more(
) {
    request::<App, _, _>(|request, _ctx| async move {
        for (path, sample, limit, more) in [
            (
                "/",
                ">View a sample report</a>",
                "Up to 5 Ad Accounts, usually one per client",
                "Need more than 5 Ad Accounts? Email us at <a href=\"mailto:hello@keysend.app\">hello@keysend.app</a>.",
            ),
            (
                "/ro",
                ">Vedeți un exemplu de raport</a>",
                "Cel mult 5 conturi de publicitate Google Ads",
                "Aveți nevoie de mai mult de 5 conturi de publicitate? Scrieți-ne la <a href=\"mailto:hello@keysend.app\">hello@keysend.app</a>.",
            ),
        ] {
            let body = request.get(path).await.text();
            let hero = body.split(" id=\"how\"").next().unwrap();
            assert!(
                hero.contains(&format!("<a href=\"/sample-report.pdf\"{sample}")),
                "{path}"
            );
            let price = body
                .split(" id=\"price\"")
                .nth(1)
                .unwrap()
                .split("</section>")
                .next()
                .unwrap();
            assert!(price.contains("href=\"/sample-report.pdf\""), "{path}");
            assert!(price.contains(limit), "{path}");
            assert!(price.contains(more), "{path}");
        }
        assert_eq!(
            request.get("/sample-report.pdf").await.header("content-type"),
            "application/pdf"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_founder_note_has_a_photo_that_loads_and_an_address_to_write_to() {
    request::<App, _, _>(|request, _ctx| async move {
        let body = request.get("/").await.text();
        let note = body
            .split("aria-labelledby=\"founder\">")
            .nth(1)
            .unwrap()
            .split("</section>")
            .next()
            .unwrap();
        assert!(note.contains("href=\"mailto:hello@keysend.app\""));
        let photo = attribute_values(note, "src")[0];
        assert!(photo.ends_with("/images/paul-badarau.webp"));
        assert_eq!(request.get(photo).await.status_code(), 200);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn home_names_its_publisher_and_its_questions_as_structured_data() {
    request::<App, _, _>(|request, _ctx| async move {
        let body = request.get("/").await.text();
        let blocks = structured_data(&body);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["@type"], "SoftwareApplication");
        assert_eq!(
            blocks[0]["publisher"]["@id"],
            "https://keysend.app/#organization"
        );
        let graph = blocks[1]["@graph"].as_array().unwrap();
        assert_eq!(graph.len(), 2);
        assert_eq!(graph[0]["@type"], "Organization");
        assert_eq!(graph[0]["@id"], "https://keysend.app/#organization");
        assert_eq!(graph[0]["legalName"], "ALPLUS TECHNOLOGIES S.R.L.");
        let resource = request
            .get(&Catalog::load().published_kind("guide")[0].path)
            .await
            .text();
        assert_eq!(structured_data(&resource)[0]["@graph"][0], graph[0]);

        let questions = schema_questions(&blocks[1]);
        assert_eq!(questions, shown_questions(&body));
        assert_eq!(questions.len(), 10);
        assert_eq!(
            questions[0],
            (
                "Can KeySend change my ads or my budget?".to_owned(),
                "No. KeySend only reads the Ad Account. When a Recommendation says to change something, you make the change yourself in Google Ads.".to_owned()
            )
        );
        assert_eq!(
            questions[1],
            (
                "What counts as an Ad Account?".to_owned(),
                "One Google Ads account that you connect, usually one client. A manager account does not count. You choose the client accounts under it.".to_owned()
            )
        );
        assert_eq!(
            questions[5],
            (
                "Google says that KeySend can edit my Ad Account. Is that true?".to_owned(),
                "Google words this permission broadly for every app, and it covers reading and editing. KeySend only sends read requests today. You can remove the permission at any time in your Google account.".to_owned()
            )
        );
        assert_eq!(
            questions[7],
            (
                "Does it work with Meta or other ad platforms?".to_owned(),
                "No. KeySend reads Google Ads only.".to_owned()
            )
        );
        assert_eq!(
            questions[8].1,
            "Download a client's Export as a PDF with your name, logo and accent colour, and send it yourself. One Subscription covers up to 5 Ad Accounts. Clients have no login."
        );
    })
    .await;
}

struct AudiencePage {
    path: &'static str,
    source: &'static str,
    crumb: &'static str,
}

const AUDIENCE_PAGES: [AudiencePage; 3] = [
    AudiencePage {
        path: "/for/business-owners",
        source: "for-business-owners",
        crumb: "For business owners",
    },
    AudiencePage {
        path: "/for/agency-clients",
        source: "for-agency-clients",
        crumb: "For agency clients",
    },
    AudiencePage {
        path: "/for/marketing-managers",
        source: "for-marketing-managers",
        crumb: "For marketing managers",
    },
];

#[tokio::test]
#[serial]
async fn each_audience_page_leads_to_sign_in_with_its_source_and_describes_itself_to_search_engines(
) {
    request::<App, _, _>(|request, _ctx| async move {
        for AudiencePage {
            path,
            source,
            crumb,
        } in AUDIENCE_PAGES
        {
            let response = request.get(path).await;
            assert_eq!(response.status_code(), 200, "{path}");
            let body = response.text();
            let url = format!("https://keysend.app{path}");
            let title = body
                .split("<title>")
                .nth(1)
                .unwrap()
                .split(" | KeySend</title>")
                .next()
                .unwrap();
            let description = attribute_values(
                body.split("<meta name=\"description\"").nth(1).unwrap(),
                "content",
            )[0];

            assert_eq!(
                body.matches(&format!("href=\"/sign-in?from={source}\"")).count(),
                4,
                "{path}"
            );
            assert_eq!(body.matches("href=\"/sign-in").count(), 5, "{path}");
            for expected in [
                format!("<link rel=\"canonical\" href=\"{url}\">"),
                format!("<meta property=\"og:url\" content=\"{url}\">"),
                "14 days".to_owned(),
                "$49".to_owned(),
                "per month, plus applicable tax, for up to 5 Ad Accounts.".to_owned(),
                "href=\"/resources\"".to_owned(),
                "class=\"campaign-name\">Search - Emergency</".to_owned(),
            ] {
                assert!(body.contains(&expected), "{path}: {expected}");
            }
            for gone in [
                    "invented dental practice",
                "<button",
                "<form",
                "/ad-accounts",
                "noindex",
                " style=\"",
                "<a class=\"campaign-name\"",
            ] {
                assert!(!body.contains(gone), "{path}: {gone}");
            }
            assert_eq!(body.matches("<h1").count(), 1, "{path}");

            let ids = attribute_values(&body, "id");
            for id in &ids {
                assert_eq!(ids.iter().filter(|other| *other == id).count(), 1, "{path} {id}");
            }
            for label in attribute_values(&body, "aria-labelledby") {
                assert!(ids.contains(&label), "{path}: aria-labelledby names {label}");
            }
            let others = body
                .split(" id=\"others\"")
                .nth(1)
                .unwrap()
                .split("</nav>")
                .next()
                .unwrap();
            for other in AUDIENCE_PATHS {
                assert_eq!(
                    others.contains(&format!("href=\"{other}\"")),
                    other != path,
                    "{path} {other}"
                );
            }

            let blocks = structured_data(&body);
            assert_eq!(blocks.len(), 1, "{path}");
            let graph = blocks[0]["@graph"].as_array().unwrap();
            let types: Vec<_> = graph
                .iter()
                .map(|node| node["@type"].as_str().unwrap())
                .collect();
            assert_eq!(
                types,
                ["Organization", "WebPage", "BreadcrumbList", "FAQPage"],
                "{path}"
            );
            assert_eq!(graph[0]["@id"], "https://keysend.app/#organization");
            assert_eq!(graph[1]["url"], url.as_str());
            assert_eq!(graph[1]["name"], title, "{path}");
            assert_eq!(graph[1]["description"], description, "{path}");
            assert_eq!(
                graph[1]["publisher"]["@id"],
                "https://keysend.app/#organization"
            );
            assert_eq!(
                graph[2]["itemListElement"],
                serde_json::json!([
                    {"@type": "ListItem", "position": 1, "name": "KeySend", "item": "https://keysend.app/"},
                    {"@type": "ListItem", "position": 2, "name": crumb, "item": url},
                ])
            );
            let questions = schema_questions(&blocks[0]);
            assert!(!questions.is_empty(), "{path}");
            assert_eq!(questions, shown_questions(&body), "{path}");
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn no_public_page_states_the_unsourced_number_and_each_button_names_a_known_source() {
    request::<App, _, _>(|request, _ctx| async move {
        let mut sources = std::collections::BTreeSet::new();
        let catalog = Catalog::load();
        for path in [
            "/",
            "/ro",
            "/for/business-owners",
            "/for/agency-clients",
            "/for/marketing-managers",
            "/privacy",
            "/terms",
            "/resources",
            catalog.published_kind("guide")[0].path.as_str(),
            "/sign-in",
        ] {
            let body = request.get(path).await.text();
            assert!(!body.contains("73 more times"), "{path}");
            for link in body.split("<a ").skip(1) {
                let tag = format!(" {}", link.split('>').next().unwrap());
                let Some(href) = attribute_values(&tag, "href").into_iter().next() else {
                    continue;
                };
                if !href.starts_with("/sign-in") {
                    continue;
                }
                match href.strip_prefix("/sign-in?from=") {
                    Some(source) => {
                        assert_eq!(
                            keysend::identity::SignUpSource::parse(source)
                                .as_ref()
                                .map(keysend::identity::SignUpSource::as_str),
                            Some(source),
                            "{path}"
                        );
                        sources.insert(source.to_owned());
                    }
                    None => {
                        assert_eq!(href, "/sign-in", "{path}");
                        assert!(
                            !tag.contains("class=\"btn"),
                            "{path}: a button has no source"
                        );
                    }
                }
            }
        }
        let mut expected = [
            "for-agency-clients",
            "for-business-owners",
            "for-marketing-managers",
            "home-closing",
            "home-hero",
            "home-nav",
            "home-price",
            "nav",
            "home-closing-ro",
            "home-hero-ro",
            "home-nav-ro",
            "home-price-ro",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<std::collections::BTreeSet<_>>();
        expected.insert(
            keysend::identity::SignUpSource::resource(&catalog.published_kind("guide")[0].path)
                .as_str()
                .to_owned(),
        );
        assert_eq!(sources, expected);
        for text in [
            keysend::language::en::TEXT.home_pain_clicks_text,
            keysend::language::ro::TEXT.home_pain_clicks_text,
            keysend::language::fr::TEXT.home_pain_clicks_text,
            keysend::language::de::TEXT.home_pain_clicks_text,
            keysend::language::es::TEXT.home_pain_clicks_text,
            keysend::language::it::TEXT.home_pain_clicks_text,
            keysend::language::pt::TEXT.home_pain_clicks_text,
        ] {
            assert!(!text.contains("73"), "{text}");
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn each_resource_page_ends_its_article_with_one_call_to_action() {
    request::<App, _, _>(|request, _ctx| async move {
        let catalog = Catalog::load();
        for kind in ["guide", "tool"] {
            let path = catalog.published_kind(kind)[0].path.as_str();
            let body = request.get(path).await.text();
            assert_eq!(
                body.matches("aria-labelledby=\"try-keysend\"").count(),
                1,
                "{path}"
            );
            let action = body
                .split("</article>")
                .nth(1)
                .unwrap()
                .split("</aside>")
                .next()
                .unwrap();
            assert!(action.contains("aria-labelledby=\"try-keysend\""), "{path}");
            assert_eq!(
                action
                    .matches(&format!(
                        "href=\"/sign-in?from={}\"",
                        keysend::identity::SignUpSource::resource(path).as_str()
                    ))
                    .count(),
                1,
                "{path}"
            );
            assert_eq!(body.matches("</article>").count(), 1, "{path}");

            let ids = attribute_values(&body, "id");
            for id in &ids {
                assert_eq!(
                    ids.iter().filter(|other| *other == id).count(),
                    1,
                    "{path} {id}"
                );
            }
            assert_eq!(body.matches("<h1").count(), 1, "{path}");
        }
        let index = request.get("/resources").await.text();
        assert!(!index.contains("try-keysend"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn sign_in_explains_the_google_permission_and_carries_a_known_source() {
    request::<App, _, _>(|request, _ctx| async move {
        let body = request.get("/sign-in").await.text();
        assert!(body.contains("href=\"/privacy#read\""));
        assert!(body.find("href=\"/auth/google/start\"").unwrap() < body.find("href=\"/privacy#read\"").unwrap());

        let known = request.get("/sign-in?from=for-business-owners").await;
        assert_eq!(known.status_code(), 200);
        assert_eq!(known.header("cache-control"), "no-store");
        assert!(known.text().contains(
            "<a class=\"btn btn-primary btn-lg\" href=\"/auth/google/start?from=for-business-owners\">Sign in with Google</a>"
        ));
        for unknown in [
            "/sign-in?from=freelancers",
            "/sign-in?from=%22%3E%3Cscript%3E",
            "/sign-in?from=",
            "/sign-in?from=home-hero&from=x",
            "/sign-in?other=1",
        ] {
            let response = request.get(unknown).await;
            assert_eq!(response.status_code(), 200, "{unknown}");
            let body = response.text();
            assert!(
                body.contains(
                    "<a class=\"btn btn-primary btn-lg\" href=\"/auth/google/start\">Sign in with Google</a>"
                ),
                "{unknown}"
            );
            assert!(!body.contains("?from="), "{unknown}");
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_visitor_reads_the_romanian_home_page_directly_and_english_stays_separate() {
    request::<App, _, _>(|request, _ctx| async move {
        for path in ["/ro", "/", "/ro"] {
            let response = request.get(path).await;
            assert_eq!(response.status_code(), 200, "{path}");
            assert_publicly_cached(response.header("cache-control").to_str().unwrap());
            let body = response.text();
            let (language, canonical) = if path == "/ro" {
                ("ro", "https://keysend.app/ro")
            } else {
                ("en", "https://keysend.app/")
            };
            assert!(body.contains(&format!("<html lang=\"{language}\">")));
            assert!(body.contains(&format!("<link rel=\"canonical\" href=\"{canonical}\">")));
            assert!(body.contains(&format!(
                "<meta property=\"og:url\" content=\"{canonical}\">"
            )));
            for (code, url) in [
                ("en", "https://keysend.app/"),
                ("ro", "https://keysend.app/ro"),
                ("x-default", "https://keysend.app/"),
            ] {
                assert!(body.contains(&format!("hreflang=\"{code}\" href=\"{url}\"")));
            }
            assert!(!body.contains("noindex"));
            for source in ["home-nav", "home-hero", "home-price", "home-closing"] {
                let suffix = if language == "ro" { "-ro" } else { "" };
                assert!(body.contains(&format!("href=\"/sign-in?from={source}{suffix}\"")));
            }
            assert!(body.contains(&format!("\"inLanguage\":\"{language}\"")));
            if language == "ro" {
                assert!(body.contains("14 zile"));
                assert!(body.contains("conversiile au crescut de la 95 la 102"));
                assert!(!body.contains("Spend was steady this week"));
                assert!(!body.contains("Increase the daily budget"));
                assert!(
                    body.contains("Bugetul zilnic s-a schimbat de la 45,00\u{a0}$ la 20,00\u{a0}$")
                );
                assert!(!body.contains("Daily budget changed"));
                assert!(body.contains("31% din afișările eligibile estimate"));
                assert!(!body.contains("52,01"));
            } else {
                assert!(body.contains("conversions rose from 95 to 102"));
            }
        }
        for path in ["/ro?language=en", "/ro?utm_source=test"] {
            assert_eq!(request.get(path).await.header("cache-control"), "no-store");
        }
        assert_eq!(
            request
                .get("/ro")
                .add_header("Authorization", "Bearer test")
                .await
                .header("cache-control"),
            "no-store"
        );
        for path in ["/fr", "/ro/security", "/ro/for/business-owners"] {
            assert_eq!(request.get(path).await.status_code(), 404);
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn both_home_languages_link_to_the_public_code_in_the_founder_letter_and_footer() {
    request::<App, _, _>(|request, _ctx| async move {
        for (path, statement, label) in [
            ("/", "KeySend is open source.", "Source code on GitHub"),
            ("/ro", "KeySend este open source.", "Codul sursă pe GitHub"),
        ] {
            let response = request.get(path).await;
            assert_eq!(response.status_code(), 200);
            let body = response.text();
            let link = format!("<a href=\"https://github.com/abpaul/keysend\">{label}</a>");
            let founder = body
                .split("class=\"founder-note\"")
                .nth(1)
                .unwrap()
                .split("</section>")
                .next()
                .unwrap();
            assert!(founder.contains(statement), "{path}");
            assert!(founder.contains(&link), "{path}");
            let footer = body.split("<footer").nth(1).unwrap();
            assert!(footer.contains(&link), "{path}");
        }
    })
    .await;
}
