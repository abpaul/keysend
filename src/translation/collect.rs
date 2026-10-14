use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use loco_rs::Result;

use crate::{
    monitoring::UPDATES_SHOWN,
    monitoring::{charts, charts::change_label},
    workspace::{
        history::{date_of, day_number, local_time},
        monitoring::RecommendationState,
        WorkspaceStore,
    },
};

#[derive(Debug, PartialEq)]
pub struct Item {
    pub text: String,
    pub highlight: Option<String>,
    pub names: Vec<String>,
}

struct Items {
    base: Vec<String>,
    found: Vec<Item>,
}

impl Items {
    fn add(&mut self, text: &str, highlight: Option<&str>, more: &[&str]) {
        if text.trim().is_empty() {
            return;
        }
        let mut names = self.base.clone();
        names.extend(more.iter().map(|name| (*name).to_owned()));
        self.found.push(Item {
            text: text.to_owned(),
            highlight: highlight.map(str::to_owned),
            names,
        });
    }
}

fn key_names(key: &str) -> Vec<&str> {
    key.split(':').skip(1).collect()
}

pub async fn shown(store: &WorkspaceStore<'_>, id: &str, now: DateTime<Utc>) -> Result<Vec<Item>> {
    let campaigns = store.campaigns(id).await?;
    let account = store
        .ad_accounts()
        .await?
        .into_iter()
        .find(|ad| ad.customer_id == id)
        .map(|ad| ad.name)
        .unwrap_or_default();
    let mut base = campaigns
        .iter()
        .map(|campaign| campaign.name.clone())
        .collect::<Vec<_>>();
    base.push(account);
    let mut items = Items {
        base,
        found: Vec::new(),
    };

    let updates = store.recent_updates(id, UPDATES_SHOWN).await?;
    let detected = |update: &crate::monitoring::update::Update| {
        update
            .detection
            .campaigns
            .iter()
            .map(|campaign| campaign.name.clone())
            .collect::<Vec<_>>()
    };
    if let Some(latest) = updates.first() {
        let names = detected(latest);
        let names = names.iter().map(String::as_str).collect::<Vec<_>>();
        items.add(&latest.summary, latest.highlight.as_deref(), &names);
        if let Some(cause) = &latest.cause {
            items.add(&cause.evidence, None, &names);
        }
        for line in &latest.lines {
            items.add(&line.text, None, &names);
            if let Some(cause) = &line.cause {
                items.add(&cause.evidence, None, &names);
            }
        }
    }

    let findings = store.monitoring_findings(id).await?;
    let subjects = |key: &str| {
        findings
            .iter()
            .find(|finding| finding.rule.key == key)
            .map(|finding| {
                finding
                    .rule
                    .subjects
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    for found in store.recommendations_with_findings(id).await? {
        if found.recommendation.state != RecommendationState::Open {
            continue;
        }
        let key = found.recommendation.finding_key.as_str();
        let mut names = subjects(key);
        names.extend(key_names(key));
        for text in [
            &found.recommendation.action,
            &found.recommendation.reason,
            &found.recommendation.evidence,
        ] {
            items.add(text, None, &names);
        }
        if let Some(detected) = &found.finding {
            items.add(&detected.title, None, &names);
        }
    }
    for finding in &findings {
        let mut names = finding
            .rule
            .subjects
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        names.extend(key_names(&finding.rule.key));
        items.add(&finding.rule.detail, None, &names);
        items.add(&finding.rule.title, None, &names);
    }

    if let Some((currency, zone)) = store.ad_account_metadata(id).await? {
        let today = day_number(local_time(now, &zone)?.date_naive());
        let from = date_of(today - charts::TREND_DAYS);
        let to = date_of(today);
        if let Some((from, to)) = from.zip(to) {
            let names = campaigns
                .iter()
                .map(|campaign| (campaign.id, campaign.name.clone()))
                .collect::<BTreeMap<_, _>>();
            for change in store.account_changes(id, from, to).await? {
                if let Some(readable) = crate::google_ads::change_text(&change, &names, &currency) {
                    items.add(&readable.text, None, &[]);
                    items.add(&change_label(&readable.text), None, &[]);
                }
            }
        }
    }

    for update in updates.iter().skip(1) {
        let names = detected(update);
        let names = names.iter().map(String::as_str).collect::<Vec<_>>();
        items.add(&update.summary, None, &names);
        if let Some(cause) = &update.cause {
            items.add(&cause.evidence, None, &names);
        }
        for line in &update.lines {
            items.add(&line.text, None, &names);
            if let Some(cause) = &line.cause {
                items.add(&cause.evidence, None, &names);
            }
        }
    }
    Ok(items.found)
}
