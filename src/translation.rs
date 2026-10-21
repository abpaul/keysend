pub mod collect;
pub mod mask;
mod request;

use std::collections::HashMap;

use loco_rs::Result;
use sha2::{Digest, Sha256};

use crate::{
    language::Language,
    monitoring::update::Update,
    settings::Settings,
    workspace::{
        monitoring::{Recommendation, RecommendationState, RecommendationWithFinding},
        WorkspaceStore,
    },
};

use mask::{Account, Masked};

pub fn assessment_texts(assessment: &crate::monitoring::assessment::Assessment) -> Vec<&str> {
    std::iter::once(assessment.headline.as_str())
        .chain(
            assessment
                .worked
                .iter()
                .chain(&assessment.attention)
                .flat_map(|point| {
                    std::iter::once(point.observation.as_str()).chain(point.explanation.as_deref())
                }),
        )
        .chain(assessment.next.iter().map(String::as_str))
        .chain(assessment.market.as_deref())
        .filter(|text| !text.trim().is_empty())
        .collect()
}

pub const MAX_TEXTS: usize = 240;

pub fn hash(english: &str) -> String {
    Sha256::digest(english.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn highlight_source(summary: &str, highlight: &str) -> String {
    format!("{summary}\u{1f}{highlight}")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shown<'a> {
    pub text: &'a str,
    pub english: bool,
}

#[derive(Default)]
pub struct Wanted(Vec<String>);

impl Wanted {
    pub fn update(&mut self, update: &Update) -> &mut Self {
        self.text(&update.summary)
            .highlight(&update.summary, update.highlight.as_deref());
        if let Some(cause) = &update.cause {
            self.text(&cause.evidence);
        }
        for line in &update.lines {
            self.text(&line.text);
            if let Some(cause) = &line.cause {
                self.text(&cause.evidence);
            }
        }
        self
    }

    pub fn assessment(
        &mut self,
        assessment: &crate::monitoring::assessment::Assessment,
    ) -> &mut Self {
        for text in assessment_texts(assessment) {
            self.text(text);
        }
        self
    }

    pub fn recommendation(&mut self, recommendation: &Recommendation) -> &mut Self {
        self.text(&recommendation.action)
            .text(&recommendation.reason)
            .text(&recommendation.evidence)
            .text(&recommendation.measure)
    }

    pub fn found(&mut self, found: &RecommendationWithFinding) -> &mut Self {
        if found.recommendation.state == RecommendationState::Open {
            self.recommendation(&found.recommendation);
            if let Some(detected) = &found.finding {
                self.text(&detected.title);
            }
        }
        self
    }

    pub fn text(&mut self, english: &str) -> &mut Self {
        self.0.push(hash(english));
        self
    }

    pub fn highlight(&mut self, summary: &str, highlight: Option<&str>) -> &mut Self {
        if let Some(highlight) = highlight {
            self.0.push(hash(&highlight_source(summary, highlight)));
        }
        self
    }
}

pub struct Translations {
    language: Language,
    found: HashMap<String, String>,
}

impl Translations {
    pub fn english() -> Self {
        Self::empty(Language::En)
    }

    pub fn empty(language: Language) -> Self {
        Self {
            language,
            found: HashMap::new(),
        }
    }

    pub async fn load(
        store: &WorkspaceStore<'_>,
        language: Language,
        wanted: Wanted,
    ) -> Result<Self> {
        if language == Language::En {
            return Ok(Self::english());
        }
        let mut hashes = wanted.0;
        hashes.sort_unstable();
        hashes.dedup();
        Ok(Self {
            language,
            found: store.translations(language, &hashes).await?,
        })
    }

    pub fn language(&self) -> Language {
        self.language
    }

    pub fn shown<'a>(&'a self, english: &'a str) -> Shown<'a> {
        if self.language == Language::En {
            return Shown {
                text: english,
                english: false,
            };
        }
        match self.found.get(&hash(english)) {
            Some(text) => Shown {
                text,
                english: false,
            },
            None => Shown {
                text: english,
                english: true,
            },
        }
    }

    pub fn highlight_of<'a>(
        &'a self,
        summary: &str,
        highlight: Option<&'a str>,
    ) -> Option<&'a str> {
        let highlight = highlight?;
        if self.language == Language::En || !self.found.contains_key(&hash(summary)) {
            return Some(highlight);
        }
        self.found
            .get(&hash(&highlight_source(summary, highlight)))
            .map(String::as_str)
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub missing: usize,
    pub requests: usize,
    pub saved: usize,
    pub rejected: usize,
}

pub async fn translate_account(
    settings: &Settings,
    store: &WorkspaceStore<'_>,
    id: &str,
    language: Language,
) -> Outcome {
    let mut outcome = Outcome::default();
    if language == Language::En {
        return outcome;
    }
    let Some(key) = settings.openai_key.as_deref() else {
        return outcome;
    };
    let items = match collect::shown(store, id, settings.now()).await {
        Ok(items) => items,
        Err(_) => {
            tracing::warn!("Translation could not read the texts of an Ad Account");
            return outcome;
        }
    };
    let hashes = items
        .iter()
        .map(|item| hash(&item.text))
        .collect::<Vec<_>>();
    let known = match store.translations(language, &hashes).await {
        Ok(known) => known,
        Err(_) => {
            tracing::warn!("Translation could not read the stored translations");
            return outcome;
        }
    };
    let mut todo = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for item in items {
        let hash = hash(&item.text);
        if known.contains_key(&hash) || !seen.insert(hash) {
            continue;
        }
        todo.push(item);
    }
    todo.truncate(MAX_TEXTS);
    outcome.missing = todo.len();
    let currency = store
        .ad_account_metadata(id)
        .await
        .ok()
        .flatten()
        .map(|(currency, _)| currency);
    let masked = todo
        .iter()
        .filter_map(|item| {
            mask::mask(
                &item.text,
                &Account {
                    names: &item.names,
                    currency: currency.as_deref(),
                },
                item.highlight.as_deref(),
            )
            .map(|masked| (item, masked))
        })
        .collect::<Vec<(&collect::Item, Masked)>>();
    let target = match language.text() {
        Ok(target) => target,
        Err(_) => return outcome,
    };
    let mut templates = masked
        .iter()
        .map(|(_, masked)| masked.template().to_owned())
        .collect::<Vec<_>>();
    templates.sort_unstable();
    templates.dedup();
    let (answers, requests) = request::translate(settings, store, key, language, &templates).await;
    outcome.requests = requests;
    let mut saved = Vec::new();
    for (item, masked) in &masked {
        let Some(answer) = answers.get(masked.template()) else {
            outcome.rejected += 1;
            continue;
        };
        match masked.restore(answer, target) {
            Ok(restored) => {
                saved.push((hash(&item.text), restored.text.clone()));
                if let (Some(highlight), Some(shown)) = (&item.highlight, &restored.highlight) {
                    saved.push((
                        hash(&highlight_source(&item.text, highlight)),
                        shown.clone(),
                    ));
                }
            }
            Err(mask::Rejected(kind)) => {
                outcome.rejected += 1;
                tracing::warn!(kind, "Translation rejected");
            }
        }
    }
    outcome.saved = saved.len();
    if store
        .save_translations(id, language, &saved, settings.now().timestamp())
        .await
        .is_err()
    {
        tracing::warn!("Translation could not be stored");
        outcome.saved = 0;
    }
    outcome
}
