use keysend::app::App;
use loco_rs::testing::prelude::*;
use serial_test::serial;

use super::catalog::{assert_publicly_cached, Catalog};

#[tokio::test]
#[serial]
async fn robots_names_the_ai_crawlers_and_keeps_the_private_paths_closed() {
    request::<App, _, _>(|request, _ctx| async move {
        let response = request.get("/robots.txt").await;
        assert_eq!(response.status_code(), 200);
        assert_eq!(response.header("content-type"), "text/plain; charset=utf-8");
        assert_eq!(
            response.text(),
            "User-agent: *\n\
             User-agent: GPTBot\n\
             User-agent: OAI-SearchBot\n\
             User-agent: ChatGPT-User\n\
             User-agent: PerplexityBot\n\
             User-agent: Perplexity-User\n\
             User-agent: ClaudeBot\n\
             User-agent: Claude-SearchBot\n\
             User-agent: Claude-User\n\
             User-agent: Google-Extended\n\
             User-agent: Bingbot\n\
             Allow: /\n\
             Disallow: /auth/\n\
             Disallow: /accounts\n\
             Disallow: /connect\n\
             Disallow: /sign-in\n\
             \n\
             Sitemap: https://keysend.app/sitemap.xml\n"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn llms_txt_describes_keysend_with_the_current_price_and_the_public_pages() {
    request::<App, _, _>(|request, _ctx| async move {
        let response = request.get("/llms.txt").await;
        assert_eq!(response.status_code(), 200);
        assert_eq!(response.header("content-type"), "text/plain; charset=utf-8");
        assert_publicly_cached(response.header("cache-control").to_str().unwrap());
        let body = response.text();
        let mut lines = body.lines();
        assert_eq!(lines.next(), Some("# KeySend"));
        assert_eq!(lines.next(), Some(""));
        assert_eq!(
            lines.next(),
            Some("> KeySend is a daily Google Ads monitoring service for business owners and marketers. It works like an analyst: it watches a Google Ads Ad Account every day and writes a short Update in plain English that says how things stand, what changed, why, and what to do.")
        );
        for line in [
            "Price: a 14-day Trial with no payment card, then a Subscription of $49 per month for up to 3 Ad Accounts. The price is the same if you spend more or less on ads.",
            "- Alerts: an email the next morning when a Campaign stops or spend doubles, and two days later when conversions go to zero.",
            "## Product",
            "- [Pricing](https://keysend.app/pricing.md): the Trial, the Subscription and the limits, as Markdown",
            "## Who it is for",
            "- [Google Ads monitoring for business owners](https://keysend.app/for/business-owners): KeySend watches your Google Ads every day and writes you one plain-English Update each morning. An Alert the next morning when a Campaign stops. Free 14-day Trial, no payment card.",
            "- [A second pair of eyes on your agency's Google Ads](https://keysend.app/for/agency-clients): See what your agency's Google Ads did, in plain English. KeySend only reads the Ad Account. Free 14-day Trial, no payment card.",
            "- [Google Ads Updates for marketing managers](https://keysend.app/for/marketing-managers): A plain-English Update every morning, a Weekly email each Monday and a branded PDF Export. KeySend only reads your Google Ads. Free 14-day Trial.",
            "## Guides",
            "## Tools",
            "## Optional",
        ] {
            assert!(body.lines().any(|found| found == line), "{line}");
        }
        assert_eq!(
            body.lines().filter(|line| line.starts_with("## ")).count(),
            5
        );
        let catalog = Catalog::load();
        for (heading, kind) in [("## Guides", "guide"), ("## Tools", "tool")] {
            let listed: Vec<&str> = body
                .lines()
                .skip_while(|line| *line != heading)
                .skip(1)
                .take_while(|line| !line.starts_with("## "))
                .filter(|line| !line.is_empty())
                .collect();
            let expected: Vec<String> = catalog
                .published_kind(kind)
                .iter()
                .map(|entry| {
                    format!(
                        "- [{}](https://keysend.app{}): {}",
                        entry.title, entry.path, entry.description
                    )
                })
                .collect();
            assert_eq!(listed, expected, "{heading}");
        }
        for entry in catalog.scheduled() {
            assert!(!body.contains(&entry.path), "{}", entry.path);
        }
        assert!(!body.contains("/for/freelancers"));
        assert!(!body.contains("/thoughts/"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn pricing_md_states_the_trial_the_price_and_the_limit() {
    request::<App, _, _>(|request, _ctx| async move {
        let response = request.get("/pricing.md").await;
        assert_eq!(response.status_code(), 200);
        assert_eq!(
            response.header("content-type"),
            "text/markdown; charset=utf-8"
        );
        assert_publicly_cached(response.header("cache-control").to_str().unwrap());
        let body = response.text();
        assert!(body.starts_with("# KeySend pricing\n\n"));
        let section = |title: &str| -> Vec<String> {
            body.lines()
                .skip_while(|line| *line != title)
                .skip(1)
                .take_while(|line| !line.starts_with("## "))
                .filter(|line| !line.is_empty())
                .map(String::from)
                .collect()
        };
        assert_eq!(
            section("## Trial"),
            [
                "- Price: $0",
                "- Length: 14 days",
                "- Payment card: not required",
                "- Limit: up to 3 Ad Accounts",
                "- Includes: everything in the Subscription",
            ]
        );
        assert_eq!(
            section("## Subscription"),
            [
                "- Price: $49 per month",
                "- Limit: up to 3 Ad Accounts",
                "- Ad spend: no limit. The price is the same if you spend more or less on ads.",
                "- Cancel: at any time",
            ]
        );
        assert_eq!(section("## Included").len(), 7);
        assert!(body.contains("- Start the Trial: https://keysend.app/sign-in\n"));
        assert_eq!(
            request
                .get("/pricing.md")
                .add_header("Authorization", "Bearer cache-test")
                .await
                .header("cache-control"),
            "no-store"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn sitemap_dates_each_resource_and_each_index_with_its_newest_entry() {
    request::<App, _, _>(|request, _ctx| async move {
        let response = request.get("/sitemap.xml").await;
        assert_eq!(response.status_code(), 200);
        assert_eq!(
            response.header("content-type"),
            "application/xml; charset=utf-8"
        );
        let body = response.text();
        for url in [
            "<url><loc>https://keysend.app/</loc></url>",
            "<url><loc>https://keysend.app/privacy</loc></url>",
            "<url><loc>https://keysend.app/terms</loc></url>",
        ] {
            assert!(body.contains(url), "{url}");
        }
        let catalog = Catalog::load();
        let dated = |path: &str, at: String| {
            format!("<url><loc>https://keysend.app{path}</loc><lastmod>{at}</lastmod></url>")
        };
        for entry in catalog.published() {
            let url = dated(&entry.path, entry.published_at());
            assert!(body.contains(&url), "{url}");
        }
        for (index, kind) in [
            ("/blog", "article"),
            ("/guides", "guide"),
            ("/tools", "tool"),
        ] {
            let newest = catalog.published_kind(kind).last().unwrap().published_at();
            let url = dated(index, newest);
            assert!(body.contains(&url), "{url}");
        }
        let newest = catalog.published().last().unwrap().published_at();
        let url = dated("/resources", newest);
        assert!(body.contains(&url), "{url}");
        for entry in catalog.scheduled() {
            assert!(!body.contains(&entry.path), "{}", entry.path);
        }
        assert!(!body.contains("llms.txt"));
        assert!(!body.contains("/sign-in"));
    })
    .await;
}

fn json_ld(body: &str) -> serde_json::Value {
    let start = "<script type=\"application/ld+json\">";
    assert_eq!(body.matches(start).count(), 1);
    let text = body.split(start).nth(1).unwrap();
    serde_json::from_str(text.split("</script>").next().unwrap()).unwrap()
}

#[tokio::test]
#[serial]
async fn each_kind_of_resource_page_has_structured_data_and_open_graph_tags() {
    request::<App, _, _>(|request, _ctx| async move {
        let catalog = Catalog::load();
        for (kind, page_type, index_name, og_type) in [
            ("article", "BlogPosting", "Blog", "article"),
            ("guide", "Article", "Guides", "article"),
            ("tool", "WebPage", "Tools", "website"),
        ] {
            let entry = catalog.published_kind(kind)[0];
            let (path, title) = (entry.path.as_str(), entry.title.as_str());
            let published = entry.published_at();
            let response = request.get(path).await;
            assert_eq!(response.status_code(), 200, "{path}");
            let body = response.text();
            let url = format!("https://keysend.app{path}");
            let data = json_ld(&body);
            let graph = data["@graph"].as_array().unwrap();
            assert_eq!(graph[0]["@type"], "Organization", "{path}");
            assert_eq!(graph[0]["name"], "KeySend", "{path}");
            assert_eq!(
                graph[0]["logo"], "https://keysend.app/static/images/logo.svg",
                "{path}"
            );
            assert_eq!(graph[1]["@type"], page_type, "{path}");
            assert_eq!(graph[1]["headline"], title, "{path}");
            assert_eq!(graph[1]["url"], url.as_str(), "{path}");
            assert_eq!(graph[1]["datePublished"], graph[1]["dateModified"]);
            assert_eq!(graph[1]["datePublished"], published.as_str(), "{path}");
            assert_eq!(graph[2]["@type"], "BreadcrumbList", "{path}");
            assert_eq!(graph[2]["itemListElement"][1]["name"], index_name);
            assert_eq!(graph[2]["itemListElement"][2]["item"], url.as_str());
            assert!(
                body.contains(&format!(">{index_name}</a></nav>")),
                "the breadcrumb on the page matches the data: {path}"
            );
            for tag in [
                format!("<meta property=\"og:type\" content=\"{og_type}\">"),
                format!("<meta property=\"og:url\" content=\"{url}\">"),
                format!("<meta property=\"og:title\" content=\"{title}\">"),
                "<meta property=\"og:site_name\" content=\"KeySend\">".to_owned(),
            ] {
                assert!(body.contains(&tag), "{path}: {tag}");
            }
            assert_eq!(
                body.contains(&format!(
                    "<meta property=\"article:published_time\" content=\"{published}\">"
                )),
                og_type == "article",
                "{path}"
            );
            assert_eq!(
                body.contains("article:published_time"),
                og_type == "article",
                "{path}"
            );
        }
        assert_eq!(
            request.get("/static/images/logo.svg").await.status_code(),
            200
        );
        let index = request.get("/resources").await.text();
        assert!(
            index.contains("<meta property=\"og:url\" content=\"https://keysend.app/resources\">")
        );
    })
    .await;
}
