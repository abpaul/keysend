use std::sync::Arc;

use axum::{extract::OriginalUri, http::header, Extension};
use chrono::{DateTime, SecondsFormat, Utc};
use loco_rs::prelude::*;

use super::page::AUDIENCES;
use crate::{
    app::not_found,
    content::{self, Catalog},
    identity::SignUpSource,
    language::request_text,
    monitoring::access,
};

async fn page(
    ViewEngine(view): ViewEngine<TeraView>,
    Extension(catalog): Extension<Arc<Catalog>>,
    OriginalUri(uri): OriginalUri,
) -> Result<Response> {
    let now = crate::settings::wall_clock();
    let path = uri.path();
    if let Some(content::Index {
        kind, title_key, ..
    }) = content::INDEXES.iter().find(|index| index.path == path)
    {
        let title = request_text()?.get(title_key, &[])?;
        let entries: Vec<_> = catalog
            .visible(now)
            .iter()
            .rev()
            .filter(|entry| kind.is_none_or(|kind| entry.kind == kind))
            .collect();
        let noindex = entries.is_empty();
        return format::render().view(
            &view,
            "content/index.html",
            data!({"title":title,"path":path,"entries":entries,"noindex":noindex}),
        );
    }
    let Some(entry) = catalog.find(path, now) else {
        return Ok(not_found());
    };
    let visible = catalog.visible(now);
    let mut related: Vec<&content::Entry> = Vec::new();
    let chosen = entry
        .related
        .iter()
        .filter_map(|path| catalog.find(path, now));
    let same_cluster = visible
        .iter()
        .rev()
        .filter(|other| other.cluster == entry.cluster);
    for other in chosen.chain(same_cluster) {
        if related.len() == 4 {
            break;
        }
        if other.path != entry.path && !related.iter().any(|seen| seen.path == other.path) {
            related.push(other);
        }
    }
    let cta_link = entry
        .cta
        .link
        .as_ref()
        .filter(|link| catalog.linkable(&link.path, now));
    let source = SignUpSource::resource(&entry.path);
    format::render().view(&view, "content/page.html", data!({"entry":entry,"related":related,"cta_link":cta_link,"source":source.as_str(),"schema":entry.structured_data(),"trial_days":access::TRIAL_SECONDS / 86_400,"calculator_js":if entry.kind == "tool" {catalog.calculator_js.as_str()} else {""}}))
}

async fn sitemap(Extension(catalog): Extension<Arc<Catalog>>) -> Result<Response> {
    let visible = catalog.visible(crate::settings::wall_clock());
    let mut pages: Vec<(&str, Option<DateTime<Utc>>)> = vec![
        ("/", None),
        ("/ro", None),
        ("/privacy", None),
        ("/terms", None),
        (super::page::SECURITY_PATH, None),
    ];
    pages.extend(AUDIENCES.iter().map(|page| (page.path, None)));
    pages.extend(content::INDEXES.iter().filter_map(|index| {
        visible
            .iter()
            .filter(|entry| index.kind.is_none_or(|kind| entry.kind == kind))
            .map(content::Entry::modified_at)
            .max()
            .map(|newest| (index.path, Some(newest)))
    }));
    pages.extend(
        visible
            .iter()
            .map(|entry| (entry.path.as_str(), Some(entry.modified_at()))),
    );
    let urls = pages
        .into_iter()
        .map(|(path, modified)| {
            let alternates = if matches!(path, "/" | "/ro") {
                format!(
                    "<xhtml:link rel=\"alternate\" hreflang=\"en\" href=\"{0}/\"/><xhtml:link rel=\"alternate\" hreflang=\"ro\" href=\"{0}/ro\"/><xhtml:link rel=\"alternate\" hreflang=\"x-default\" href=\"{0}/\"/>",
                    content::ORIGIN
                )
            } else {
                String::new()
            };
            format!(
                "<url><loc>{}{}</loc>{}{alternates}</url>",
                content::ORIGIN,
                content::escape(path),
                modified.map_or_else(String::new, |at| format!(
                    "<lastmod>{}</lastmod>",
                    at.to_rfc3339_opts(SecondsFormat::Secs, true)
                ))
            )
        })
        .collect::<String>();
    Ok(([(header::CONTENT_TYPE, "application/xml; charset=utf-8")], format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\" xmlns:xhtml=\"http://www.w3.org/1999/xhtml\">{urls}</urlset>")).into_response())
}

async fn feed(Extension(catalog): Extension<Arc<Catalog>>) -> Result<Response> {
    let items = catalog.visible(crate::settings::wall_clock()).iter().rev().filter(|entry| matches!(entry.kind.as_str(), "article" | "guide")).take(30).map(|entry| format!("<item><title>{}</title><link>https://keysend.app{}</link><guid isPermaLink=\"true\">https://keysend.app{}</guid><pubDate>{}</pubDate><description>{}</description></item>",content::escape(&entry.title),content::escape(&entry.path),content::escape(&entry.path),entry.publish_at.to_rfc2822(),content::escape(&entry.description))).collect::<String>();
    Ok(([(header::CONTENT_TYPE,"application/rss+xml; charset=utf-8")],format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><rss version=\"2.0\"><channel><title>KeySend</title><link>https://keysend.app/resources</link><description>Practical Google Ads reporting</description>{items}</channel></rss>")).into_response())
}

/// A crawler that finds a group with its own name ignores the `*` group, so the named crawlers
/// and `*` share one group and the private paths stay disallowed for all of them.
const ROBOTS: &str = "User-agent: *
User-agent: GPTBot
User-agent: OAI-SearchBot
User-agent: ChatGPT-User
User-agent: PerplexityBot
User-agent: Perplexity-User
User-agent: ClaudeBot
User-agent: Claude-SearchBot
User-agent: Claude-User
User-agent: Google-Extended
User-agent: Bingbot
Allow: /
Disallow: /auth/
Disallow: /accounts
Disallow: /connect
Disallow: /sign-in
Disallow: /admin

Sitemap: https://keysend.app/sitemap.xml
";

async fn sample_report() -> Result<Response> {
    Ok((
        [
            (header::CONTENT_TYPE, "application/pdf"),
            (
                header::CONTENT_DISPOSITION,
                "inline; filename=\"keysend-sample-report.pdf\"",
            ),
        ],
        include_bytes!("../../assets/static/sample-report.pdf").as_slice(),
    )
        .into_response())
}

async fn robots() -> Result<Response> {
    Ok((
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        ROBOTS,
    )
        .into_response())
}

fn usd(cents: i64) -> String {
    if cents % 100 == 0 {
        format!("${}", cents / 100)
    } else {
        format!("${}.{:02}", cents / 100, cents % 100)
    }
}

const INCLUDED: &str = "- Update: a daily overview in plain English. It shows performance changes and the available evidence, with possible causes distinguished from contributions.
- Dashboard: what you spent, what you got and what each result cost, then charts that each state their main point.
- Recommendations: each one names the problem, what it costs, the change to make and the numbers behind it.
- Deep check: once a week, a review of the search terms, keywords, landing pages and ads that take the most money.
- Alerts: selected problems, including Campaign stops and qualifying spend increases, can trigger an email the next morning. Zero-conversion Alerts allow two days for delayed data.
- Weekly email: each Monday, what you spent and what you got, compared with the 7 days before.
- Export: a PDF of the Ad Account or of one Campaign for the last 7 or 30 days, with your name, logo and accent colour.
";

async fn llms(Extension(catalog): Extension<Arc<Catalog>>) -> Result<Response> {
    let origin = content::ORIGIN;
    let price = usd(crate::billing::SUBSCRIPTION_USD_CENTS);
    let trial_days = access::TRIAL_SECONDS / 86_400;
    let limit = access::AD_ACCOUNT_LIMIT;
    let text = request_text()?;
    let audiences = AUDIENCES
        .iter()
        .map(|page| {
            Ok(format!(
                "- [{}]({origin}{}): {}\n",
                text.get(page.title, &[])?,
                page.path,
                page.described(text)?
            ))
        })
        .collect::<Result<String>>()?;
    let list = |kind: &str| {
        catalog
            .visible(crate::settings::wall_clock())
            .iter()
            .filter(|entry| entry.kind == kind)
            .map(|entry| {
                format!(
                    "- [{}]({origin}{}): {}\n",
                    entry.title, entry.path, entry.description
                )
            })
            .collect::<String>()
    };
    let body = format!(
        "# KeySend

> KeySend is a Google Ads monitoring and reporting service for consultants and freelancers. Daily Updates show performance movements, a weekly Deep check writes an assessment with Recommendations, and branded PDF Exports are ready to send to clients. Business owners can also use it for their own Ad Accounts.

KeySend works with Google Ads only. It reads the Ad Account. When a Recommendation says to change something, you make the change yourself in Google Ads.

{INCLUDED}
Price: a {trial_days}-day Trial with no payment card, then a voluntary Subscription of {price} per month, plus applicable tax, for up to {limit} Ad Accounts. The price is the same if you spend more or less on ads.

KeySend is run by ALPLUS TECHNOLOGIES S.R.L., a company registered in Romania. Contact: hello@keysend.app.

## Product

- [Home]({origin}/): what KeySend does, with screenshots and questions
- [Pricing]({origin}/pricing.md): the Trial, the Subscription and the limits, as Markdown
- [Start the Trial]({origin}/sign-in): sign in with the Google login that you use for Google Ads
- [Privacy]({origin}/privacy): what KeySend stores and what it reads from Google Ads
- [Terms]({origin}/terms): price, payment, refunds and the operator
- [Security]({origin}/security): encryption, where the data is, how long KeySend keeps it, and the rights of the User

## Who it is for

{audiences}
## Guides

{guides}
## Tools

{tools}
## Optional

- [Resources]({origin}/resources): all articles, guides and calculators about Google Ads reporting
- [Articles]({origin}/blog): practical Google Ads reporting methods with worked examples
- [RSS feed]({origin}/feed.xml): the newest articles and guides
",
        guides = list("guide"),
        tools = list("tool"),
    );
    Ok(([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], body).into_response())
}

async fn pricing() -> Result<Response> {
    let origin = content::ORIGIN;
    let price = usd(crate::billing::SUBSCRIPTION_USD_CENTS);
    let trial_days = access::TRIAL_SECONDS / 86_400;
    let limit = access::AD_ACCOUNT_LIMIT;
    let body = format!(
        "# KeySend pricing

KeySend is a daily Google Ads monitoring service. It has one Trial and one Subscription. Prices are in US dollars.

## Trial

- Price: $0
- Length: {trial_days} days
- Payment card: not required
- Limit: up to {limit} Ad Accounts
- Includes: everything in the Subscription

## Subscription

- Price: {price} per month, plus applicable tax
- Limit: up to {limit} Ad Accounts
- Ad spend: no limit. The price is the same if you spend more or less on ads.
- Cancel: at any time
- Access: until the end of the paid period after cancellation
- First payment: email within 30 days for a full refund, under the Terms

## Included

{INCLUDED}
## After the Trial

The daily Updates stop until you start the Subscription. KeySend has no payment card from the Trial and charges nothing.

## Links

- Start the Trial: {origin}/sign-in
- Terms, with payment and refunds: {origin}/terms
- Contact: hello@keysend.app
"
    );
    Ok((
        [(header::CONTENT_TYPE, "text/markdown; charset=utf-8")],
        body,
    )
        .into_response())
}

pub fn routes() -> Routes {
    let mut routes = Routes::new()
        .add("/sample-report.pdf", get(sample_report))
        .add("/sitemap.xml", get(sitemap))
        .add("/feed.xml", get(feed))
        .add("/robots.txt", get(robots))
        .add("/llms.txt", get(llms))
        .add("/pricing.md", get(pricing));
    for index in content::INDEXES {
        routes = routes.add(index.path, get(page));
    }
    for path in ["/blog/{slug}", "/guides/{slug}", "/tools/{slug}"] {
        routes = routes.add(path, get(page));
    }
    routes
}
