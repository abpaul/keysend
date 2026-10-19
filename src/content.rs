use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub struct Index {
    pub path: &'static str,
    pub kind: Option<&'static str>,
    pub title_key: &'static str,
}

const fn index(path: &'static str, kind: Option<&'static str>, title_key: &'static str) -> Index {
    Index {
        path,
        kind,
        title_key,
    }
}

pub const INDEXES: [Index; 4] = [
    index("/blog", Some("article"), "content_title_blog"),
    index("/guides", Some("guide"), "content_title_guides"),
    index("/tools", Some("tool"), "content_title_tools"),
    index("/resources", None, "content_title_resources"),
];

pub const INTENTS: [&str; 3] = ["informational", "comparison", "commercial"];

/// The pages outside the catalog that an entry may link to at any time. The content generator lists the same pages.
pub const STATIC_PAGES: [&str; 11] = [
    "/",
    "/security",
    "/privacy",
    "/terms",
    "/resources",
    "/blog",
    "/guides",
    "/tools",
    "/for/business-owners",
    "/for/agency-clients",
    "/for/marketing-managers",
];

pub const ORIGIN: &str = "https://keysend.app";
pub const CONTACT: &str = "hello@keysend.app";

#[derive(Deserialize)]
pub struct Catalog {
    schema_version: u32,
    entries: Vec<Entry>,
    pub calculator_js: String,
}

#[derive(Deserialize, Serialize)]
pub struct Entry {
    pub path: String,
    pub title: String,
    pub seo_title: String,
    pub description: String,
    pub publish_at: DateTime<Utc>,
    pub updated_at: Option<DateTime<Utc>>,
    pub kind: String,
    pub cluster: String,
    pub intent: String,
    pub related: Vec<String>,
    pub cta: Cta,
    pub body_html: String,
}

#[derive(Deserialize, Serialize)]
pub struct Cta {
    pub heading: String,
    pub text: String,
    pub link: Option<CtaLink>,
}

#[derive(Deserialize, Serialize)]
pub struct CtaLink {
    pub path: String,
    pub label: String,
}

impl Entry {
    pub fn modified_at(&self) -> DateTime<Utc> {
        self.updated_at.unwrap_or(self.publish_at)
    }

    pub fn structured_data(&self) -> String {
        let (page_type, index, index_name) = match self.kind.as_str() {
            "article" => ("BlogPosting", "/blog", "Blog"),
            "guide" => ("Article", "/guides", "Guides"),
            _ => ("WebPage", "/tools", "Tools"),
        };
        let url = format!("{ORIGIN}{}", self.path);
        let organization = organization_id();
        let crumb = |position: u8, name: &str, item: &str| serde_json::json!({"@type": "ListItem", "position": position, "name": name, "item": item});
        script_json(&serde_json::json!({
            "@context": "https://schema.org",
            "@graph": [
                self::organization(),
                {
                    "@type": page_type,
                    "@id": format!("{url}#page"),
                    "url": url,
                    "mainEntityOfPage": url,
                    "headline": self.title,
                    "description": self.description,
                    "inLanguage": "en",
                    "datePublished": self.publish_at,
                    "dateModified": self.modified_at(),
                    "author": {"@id": organization},
                    "publisher": {"@id": organization},
                },
                {
                    "@type": "BreadcrumbList",
                    "itemListElement": [
                        crumb(1, "Resources", &format!("{ORIGIN}/resources")),
                        crumb(2, index_name, &format!("{ORIGIN}{index}")),
                        crumb(3, &self.title, &url),
                    ],
                },
            ],
        }))
    }
}

pub fn organization_id() -> String {
    format!("{ORIGIN}/#organization")
}

pub fn organization() -> serde_json::Value {
    serde_json::json!({
        "@type": "Organization",
        "@id": organization_id(),
        "name": "KeySend",
        "legalName": "ALPLUS TECHNOLOGIES S.R.L.",
        "url": format!("{ORIGIN}/"),
        "logo": format!("{ORIGIN}/static/images/logo.svg"),
        "email": CONTACT,
    })
}

pub fn script_json(value: &serde_json::Value) -> String {
    value.to_string().replace('<', "\\u003c")
}

impl Catalog {
    pub fn load() -> loco_rs::Result<Self> {
        let mut catalog: Self = serde_json::from_str(
            &std::fs::read_to_string("content/seo/catalog.json")?.replace(
                "{ad_account_limit}",
                &crate::monitoring::access::AD_ACCOUNT_LIMIT.to_string(),
            ),
        )?;
        let mut paths = HashSet::new();
        if catalog.schema_version != 2 {
            return Err(loco_rs::Error::Message(
                "Unsupported content catalog".into(),
            ));
        }
        for entry in &catalog.entries {
            let prefix = match entry.kind.as_str() {
                "article" => "/blog/",
                "guide" => "/guides/",
                "tool" => "/tools/",
                _ => return Err(loco_rs::Error::Message("Invalid content kind".into())),
            };
            let valid = entry.path.strip_prefix(prefix).is_some_and(|slug| {
                !slug.is_empty()
                    && slug.bytes().all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
                    })
            });
            if !valid || !paths.insert(&entry.path) {
                return Err(loco_rs::Error::Message("Invalid content path".into()));
            }
            if !INTENTS.contains(&entry.intent.as_str())
                || entry.updated_at.is_some_and(|at| at <= entry.publish_at)
            {
                return Err(loco_rs::Error::Message("Invalid content entry".into()));
            }
        }
        catalog.entries.sort_by_key(|entry| entry.publish_at);
        Ok(catalog)
    }

    pub fn visible(&self, now: DateTime<Utc>) -> &[Entry] {
        let count = self
            .entries
            .partition_point(|entry| entry.publish_at <= now);
        &self.entries[..count]
    }

    /// The target of a link on a visible page, when a visitor may open it now.
    pub fn linkable(&self, path: &str, now: DateTime<Utc>) -> bool {
        STATIC_PAGES.contains(&path) || self.find(path, now).is_some()
    }

    pub fn find(&self, path: &str, now: DateTime<Utc>) -> Option<&Entry> {
        self.visible(now).iter().find(|entry| entry.path == path)
    }

    pub fn cache_seconds(&self, rendered_at: DateTime<Utc>, now: DateTime<Utc>) -> i64 {
        self.entries
            .iter()
            .find(|entry| entry.publish_at > rendered_at)
            .map_or(300, |entry| {
                (entry.publish_at - now).num_seconds().clamp(0, 300)
            })
    }
}

pub fn html_page(path: &str) -> bool {
    INDEXES.iter().any(|index| path == index.path)
        || ["/blog/", "/guides/", "/tools/"]
            .iter()
            .any(|prefix| path.starts_with(prefix))
}

pub fn public_path(path: &str) -> bool {
    html_page(path)
        || matches!(
            path,
            "/sitemap.xml" | "/feed.xml" | "/robots.txt" | "/llms.txt" | "/pricing.md"
        )
}

pub fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: &str, title: &str, description: &str) -> Entry {
        Entry {
            path: "/blog/a-quoted-title".into(),
            title: title.into(),
            seo_title: title.into(),
            description: description.into(),
            publish_at: "2026-10-13T08:00:00Z".parse().unwrap(),
            updated_at: None,
            kind: kind.into(),
            cluster: "reporting-basics".into(),
            intent: "informational".into(),
            related: Vec::new(),
            cta: Cta {
                heading: String::new(),
                text: String::new(),
                link: None,
            },
            body_html: String::new(),
        }
    }

    #[test]
    fn a_public_page_is_cached_for_five_minutes_or_until_the_next_entry_publishes() {
        let publication = entry("article", "T", "D").publish_at;
        let catalog = Catalog {
            schema_version: 1,
            entries: vec![entry("article", "T", "D")],
            calculator_js: String::new(),
        };
        let at = |seconds_before: i64| publication - chrono::Duration::seconds(seconds_before);
        assert_eq!(catalog.cache_seconds(at(3600), at(3600)), 300);
        assert_eq!(catalog.cache_seconds(at(120), at(120)), 120);
        assert_eq!(catalog.cache_seconds(at(120), at(-5)), 0);
        assert_eq!(catalog.cache_seconds(at(-5), at(-5)), 300);
    }

    #[test]
    fn structured_data_cannot_close_its_script_element() {
        let text = script_json(&serde_json::json!({"name": "a</script><b>"}));
        assert_eq!(text, r#"{"name":"a\u003c/script>\u003cb>"}"#);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&text).unwrap()["name"],
            "a</script><b>"
        );
    }

    #[test]
    fn structured_data_stays_valid_json_when_the_title_has_quotes_and_markup() {
        let title = r#"The "best" report </script><b>"#;
        let text = entry("article", title, r"It's 5 > 3 & \ fine").structured_data();
        assert!(!text.contains('<'));
        let data: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(data["@context"], "https://schema.org");
        let graph = data["@graph"].as_array().unwrap();
        assert_eq!(graph.len(), 3);
        assert_eq!(graph[0]["@type"], "Organization");
        assert_eq!(graph[0]["@id"], "https://keysend.app/#organization");
        assert_eq!(graph[0]["legalName"], "ALPLUS TECHNOLOGIES S.R.L.");
        assert_eq!(graph[1]["@type"], "BlogPosting");
        assert_eq!(graph[1]["headline"], title);
        assert_eq!(graph[1]["description"], r"It's 5 > 3 & \ fine");
        assert_eq!(graph[1]["datePublished"], "2026-10-13T08:00:00Z");
        assert_eq!(graph[1]["dateModified"], "2026-10-13T08:00:00Z");
        assert_eq!(graph[1]["url"], "https://keysend.app/blog/a-quoted-title");
        assert_eq!(
            graph[1]["author"]["@id"],
            "https://keysend.app/#organization"
        );
        let crumbs = graph[2]["itemListElement"].as_array().unwrap();
        assert_eq!(crumbs[0]["item"], "https://keysend.app/resources");
        assert_eq!(crumbs[1]["name"], "Blog");
        assert_eq!(crumbs[1]["item"], "https://keysend.app/blog");
        assert_eq!(crumbs[2]["position"], 3);
        assert_eq!(crumbs[2]["name"], title);
    }

    #[test]
    fn an_edited_entry_states_when_it_was_updated() {
        let mut edited = entry("article", "T", "D");
        edited.updated_at = Some("2026-11-02T09:30:00Z".parse().unwrap());
        let data: serde_json::Value = serde_json::from_str(&edited.structured_data()).unwrap();
        assert_eq!(data["@graph"][1]["datePublished"], "2026-10-13T08:00:00Z");
        assert_eq!(data["@graph"][1]["dateModified"], "2026-11-02T09:30:00Z");
    }

    #[test]
    fn a_page_links_only_to_pages_that_are_public_when_it_is() {
        let catalog = Catalog::load().unwrap();
        for entry in &catalog.entries {
            let mut targets: Vec<&str> = entry
                .body_html
                .split("href=\"")
                .skip(1)
                .filter_map(|rest| rest.split('"').next())
                .filter(|href| href.starts_with('/'))
                .collect();
            targets.extend(entry.cta.link.as_ref().map(|link| link.path.as_str()));
            for target in targets {
                assert!(
                    catalog.linkable(target, entry.publish_at),
                    "{} links to {target} before it is public",
                    entry.path
                );
            }
        }
    }

    #[test]
    fn structured_data_names_the_type_and_the_index_of_each_kind() {
        for (kind, page_type, index) in [
            ("article", "BlogPosting", "https://keysend.app/blog"),
            ("guide", "Article", "https://keysend.app/guides"),
            ("tool", "WebPage", "https://keysend.app/tools"),
        ] {
            let data: serde_json::Value =
                serde_json::from_str(&entry(kind, "T", "D").structured_data()).unwrap();
            assert_eq!(data["@graph"][1]["@type"], page_type, "{kind}");
            assert_eq!(
                data["@graph"][2]["itemListElement"][1]["item"], index,
                "{kind}"
            );
        }
    }

    #[test]
    fn every_entry_is_hidden_until_its_exact_utc_publication_time() {
        let catalog = Catalog::load().unwrap();
        assert!(!catalog.entries.is_empty());
        for entry in &catalog.entries {
            let before = entry.publish_at - chrono::Duration::seconds(1);
            assert!(catalog.find(&entry.path, before).is_none());
            assert_eq!(catalog.cache_seconds(before, before), 1);
            assert_eq!(
                catalog.find(&entry.path, entry.publish_at).unwrap().title,
                entry.title
            );
        }
        let first = catalog.entries[0].publish_at;
        assert_eq!(
            catalog.visible(first - chrono::Duration::seconds(1)).len(),
            0
        );
        assert_eq!(
            catalog.visible(first).len(),
            catalog
                .entries
                .iter()
                .filter(|entry| entry.publish_at == first)
                .count()
        );
    }
}
