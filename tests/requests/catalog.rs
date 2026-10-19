use chrono::{DateTime, SecondsFormat, Utc};

#[derive(serde::Deserialize)]
pub struct Entry {
    pub path: String,
    pub title: String,
    pub description: String,
    pub body_html: String,
    pub kind: String,
    pub publish_at: DateTime<Utc>,
    pub updated_at: Option<DateTime<Utc>>,
}

impl Entry {
    pub fn published_at(&self) -> String {
        self.publish_at.to_rfc3339_opts(SecondsFormat::Secs, true)
    }

    pub fn modified_at(&self) -> String {
        self.updated_at
            .unwrap_or(self.publish_at)
            .to_rfc3339_opts(SecondsFormat::Secs, true)
    }
}

#[derive(serde::Deserialize)]
pub struct Catalog {
    pub calculator_js: String,
    entries: Vec<Entry>,
}

impl Catalog {
    pub fn load() -> Self {
        let mut catalog: Self =
            serde_json::from_slice(&std::fs::read("content/seo/catalog.json").unwrap()).unwrap();
        catalog.entries.sort_by_key(|entry| entry.publish_at);
        catalog
    }

    pub fn published(&self) -> Vec<&Entry> {
        let now = Utc::now();
        self.entries
            .iter()
            .filter(|entry| entry.publish_at <= now)
            .collect()
    }

    pub fn scheduled(&self) -> Vec<&Entry> {
        let now = Utc::now();
        self.entries
            .iter()
            .filter(|entry| entry.publish_at > now)
            .collect()
    }

    pub fn published_kind(&self, kind: &str) -> Vec<&Entry> {
        let entries: Vec<_> = self
            .published()
            .into_iter()
            .filter(|entry| entry.kind == kind)
            .collect();
        assert!(!entries.is_empty(), "the catalog has no published {kind}");
        entries
    }
}

/// A public page is cached for 300 seconds, or only until the next entry publishes when that is sooner.
pub fn assert_publicly_cached(cache_control: &str) {
    let seconds: i64 = cache_control
        .strip_prefix("public, max-age=0, s-maxage=")
        .and_then(|rest| rest.strip_suffix(", must-revalidate"))
        .and_then(|seconds| seconds.parse().ok())
        .unwrap_or_else(|| panic!("not publicly cached: {cache_control}"));
    assert!((0..=300).contains(&seconds), "{cache_control}");
}
