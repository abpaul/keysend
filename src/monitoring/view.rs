use chrono::NaiveDate;
use loco_rs::{Error, Result};
use serde::Serialize;

use super::{
    format::{
        self, Column, CLICKS, CONVERSIONS, COST_PER_CLICK, COST_PER_CONVERSION, IMPRESSIONS, SPEND,
    },
    metrics::{sum, CampaignDay, WindowMetrics},
    tracking::ConversionTracking,
};
use crate::{
    language::{say, Text},
    workspace::history::date_of,
};

pub const STORED_UNNAMED_AD: &str = "Not available";

fn date(day: i64) -> Result<NaiveDate> {
    date_of(day).ok_or_else(|| Error::Message("Invalid day".into()))
}

pub fn long_date(text: &Text, day: i64) -> Result<String> {
    Ok(format::long_date(text, date(day)?))
}

pub fn day_month(text: &Text, day: i64) -> Result<String> {
    Ok(text.written.format.day_month(date(day)?))
}

pub fn short_date(text: &Text, day: i64) -> Result<String> {
    Ok(text.written.format.short_date(date(day)?))
}

pub enum Earlier<'a, T> {
    One(&'a T),
    Quiet(String),
}

pub fn earlier<'a, T>(
    text: &Text,
    updates: &'a [T],
    day: impl Fn(&T) -> i64,
    quiet: impl Fn(&T) -> bool,
) -> Result<Vec<Earlier<'a, T>>> {
    let mut items = Vec::new();
    let mut index = 0;
    while index < updates.len() {
        let run = updates[index..]
            .iter()
            .take_while(|update| quiet(update))
            .count();
        if run == 0 {
            items.push(Earlier::One(&updates[index]));
            index += 1;
            continue;
        }
        let (newest, oldest) = (day(&updates[index]), day(&updates[index + run - 1]));
        items.push(Earlier::Quiet(if newest == oldest {
            long_date(text, newest)?
        } else {
            span(text, oldest, newest)?
        }));
        index += run;
    }
    Ok(items)
}

pub fn iso_date(day: i64) -> Result<String> {
    Ok(date(day)?.to_string())
}

pub fn span(text: &Text, start: i64, end: i64) -> Result<String> {
    Ok(text.written.format.span(date(start)?, date(end)?))
}

#[derive(Debug, Serialize, PartialEq)]
pub struct MetricView {
    pub label: &'static str,
    pub value: String,
    pub change: Option<String>,
    pub was: String,
}

fn shown(text: &Text, value: Option<f64>, format: &dyn Fn(f64) -> String) -> String {
    value.map_or_else(|| text.written.not_available.into(), format)
}

fn metric(
    text: &Text,
    column: &Column,
    window: &WindowMetrics,
    currency: &str,
) -> Result<MetricView> {
    let (current, previous) = (
        (column.read)(&window.current),
        (column.read)(&window.previous),
    );
    let format = |value| column.show(text, value, currency);
    let (change, was) = match (current, previous) {
        (Some(current), Some(previous)) if previous != 0.0 => (
            Some(text.change(current, previous)?),
            say!(text, from_value, value = format(previous))?,
        ),
        (_, Some(previous)) => (None, say!(text, was_value, value = format(previous))?),
        (_, None) => (None, text.written.was_not_available.into()),
    };
    Ok(MetricView {
        label: (column.label)(&text.written),
        value: shown(text, current, &format),
        change,
        was,
    })
}

fn columns(judged: bool) -> [&'static Column; 4] {
    if judged {
        [&SPEND, &CONVERSIONS, &COST_PER_CONVERSION, &CLICKS]
    } else {
        [&SPEND, &CLICKS, &COST_PER_CLICK, &IMPRESSIONS]
    }
}

pub fn metrics(text: &Text, window: &WindowMetrics, currency: &str) -> Result<Vec<MetricView>> {
    columns(window.current.conversions.is_some())
        .into_iter()
        .map(|column| metric(text, column, window, currency))
        .collect()
}

pub fn metric_days(
    days: &[CampaignDay],
    start: i64,
    end: i64,
    campaign: Option<i64>,
    judged: bool,
) -> Vec<(&'static Column, Vec<Option<f64>>)> {
    let totals = (start..=end)
        .map(|day| {
            let total = sum(days, day, day, campaign);
            if judged {
                total
            } else {
                total.without_conversions()
            }
        })
        .collect::<Vec<_>>();
    columns(judged)
        .into_iter()
        .map(|column| {
            let values = totals
                .iter()
                .map(|total| (column.read)(total).filter(|value| value.is_finite()))
                .collect();
            (column, values)
        })
        .collect()
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Cell {
    pub value: String,
    pub was: String,
}

pub fn campaign_headers(text: &Text, judged: bool) -> Vec<&'static str> {
    columns(judged)[..3]
        .iter()
        .map(|column| (column.label)(&text.written))
        .collect()
}

fn cells_of(
    text: &Text,
    columns: &[&Column],
    window: &WindowMetrics,
    currency: &str,
) -> Result<Vec<Cell>> {
    columns
        .iter()
        .map(|column| {
            let format = |value| column.show(text, value, currency);
            Ok(Cell {
                value: shown(text, (column.read)(&window.current), &format),
                was: match (column.read)(&window.previous) {
                    None => text.written.was_not_available.into(),
                    Some(value) => say!(text, was_value, value = format(value))?,
                },
            })
        })
        .collect()
}

pub fn listed(campaign: &crate::workspace::history::Campaign, metrics: &WindowMetrics) -> bool {
    campaign.status == "ENABLED" || metrics.current.serving() || metrics.previous.serving()
}

pub fn campaign_cells(text: &Text, window: &WindowMetrics, currency: &str) -> Result<Vec<Cell>> {
    cells_of(
        text,
        &columns(window.current.conversions.is_some())[..3],
        window,
        currency,
    )
}

pub fn all_campaign_headers(text: &Text, judged: bool) -> Vec<&'static str> {
    format::all_columns(judged)
        .iter()
        .map(|column| (column.label)(&text.written))
        .collect()
}

pub fn all_campaign_cells(
    text: &Text,
    window: &WindowMetrics,
    currency: &str,
    judged: bool,
) -> Result<Vec<Cell>> {
    let mut cells = cells_of(text, format::all_columns(judged), window, currency)?;
    if window.current.conversions.is_none() {
        for (cell, column) in cells.iter_mut().zip(format::all_columns(judged)) {
            if (column.read)(&window.current).is_none() {
                text.written.export_not_judged.clone_into(&mut cell.value);
                cell.was.clear();
            }
        }
    }
    Ok(cells)
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Notice {
    pub title: &'static str,
    pub text: &'static str,
}

pub fn tracking_notice(
    text: &Text,
    state: ConversionTracking,
    chosen: Option<bool>,
    judged: bool,
) -> Option<Notice> {
    if judged {
        return None;
    }
    let written = &text.written;
    Some(
        if chosen == Some(false) && state == ConversionTracking::Working {
            Notice {
                title: written.notice_not_judged_title,
                text: written.notice_not_judged,
            }
        } else if state == ConversionTracking::NotSetUp {
            Notice {
                title: written.notice_not_set_up_title,
                text: written.notice_not_set_up,
            }
        } else {
            Notice {
                title: written.notice_too_few_title,
                text: written.notice_too_few,
            }
        },
    )
}

/// The notice of a Campaign: none when its results are judged, its own when only the Ad Account
/// is judged, and the notice of the Ad Account otherwise.
pub fn campaign_notice(
    text: &Text,
    account: Option<Notice>,
    campaign_judged: bool,
) -> Option<Notice> {
    if campaign_judged {
        None
    } else {
        account.or(Some(Notice {
            title: text.campaign_not_judged_title,
            text: text.campaign_not_judged_text,
        }))
    }
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Marked {
    pub before: String,
    pub mark: String,
    pub after: String,
}

pub fn marked(summary: &str, highlight: Option<&str>) -> Marked {
    let found =
        highlight.and_then(|highlight| summary.find(highlight).map(|start| (start, highlight)));
    match found {
        Some((start, highlight)) => Marked {
            before: summary[..start].into(),
            mark: highlight.into(),
            after: summary[start + highlight.len()..].into(),
        },
        None => Marked {
            before: summary.into(),
            mark: String::new(),
            after: String::new(),
        },
    }
}

pub fn first_sentence(text: &str) -> &str {
    text.find(". ").map_or(text, |end| &text[..=end])
}

/// The stored Assessment, translated for a page or an Export.
pub struct AssessmentText {
    pub result: &'static str,
    pub period: String,
    pub headline: String,
    pub sections: Vec<(&'static str, Vec<AssessmentPoint>)>,
    pub sources: Vec<String>,
}

pub struct AssessmentPoint {
    pub observation: String,
    pub explanation: Option<String>,
}

impl From<String> for AssessmentPoint {
    fn from(observation: String) -> Self {
        Self {
            observation,
            explanation: None,
        }
    }
}

impl From<&str> for AssessmentPoint {
    fn from(observation: &str) -> Self {
        observation.to_owned().into()
    }
}

pub fn confidence(text: &Text, confidence: super::recommendation::Confidence) -> &'static str {
    use super::recommendation::Confidence;
    match confidence {
        Confidence::High => text.confidence_high,
        Confidence::Medium => text.confidence_medium,
        Confidence::Low => text.confidence_low,
    }
}

/// Only verified research facts supply source hosts; paths and duplicate hosts stay out.
pub fn market_sources(research: Option<&super::business::Research>) -> Vec<String> {
    research
        .into_iter()
        .flat_map(|research| &research.facts)
        .filter(|fact| fact.verified)
        .filter_map(|fact| reqwest::Url::parse(&fact.source).ok())
        .filter_map(|url| url.host_str().map(str::to_owned))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub fn assessment_text(
    text: &Text,
    day: i64,
    assessment: &super::assessment::Assessment,
    translations: &crate::translation::Translations,
) -> Result<AssessmentText> {
    use super::assessment::Verdict;
    let written = &text.written;
    let result = match assessment.verdict {
        Verdict::Improved => written.assessment_improved,
        Verdict::Steady => written.assessment_steady,
        Verdict::Mixed => written.assessment_mixed,
        Verdict::Worse => written.assessment_worse,
        Verdict::Seasonal => written.assessment_seasonal,
        Verdict::Unclear => written.assessment_unclear,
    };
    let plain = |items: &[String]| {
        items
            .iter()
            .map(|item| AssessmentPoint {
                observation: translations.shown(item).text.into(),
                explanation: None,
            })
            .collect::<Vec<_>>()
    };
    let points = |points: &[super::claims::Point]| {
        points
            .iter()
            .map(|point| AssessmentPoint {
                observation: translations.shown(&point.observation).text.into(),
                explanation: point
                    .explanation
                    .as_ref()
                    .map(|explanation| translations.shown(explanation).text.into()),
            })
            .collect::<Vec<_>>()
    };
    let sections = [
        (written.assessment_worked, points(&assessment.worked)),
        (written.assessment_attention, points(&assessment.attention)),
        (written.assessment_next, plain(&assessment.next)),
        (
            written.assessment_market,
            plain(&assessment.market.iter().cloned().collect::<Vec<_>>()),
        ),
    ]
    .into_iter()
    .filter_map(|(label, items)| {
        let items = items
            .into_iter()
            .filter(|item| !item.observation.trim().is_empty())
            .collect::<Vec<_>>();
        (!items.is_empty()).then_some((label, items))
    })
    .collect();
    Ok(AssessmentText {
        result,
        sources: Vec::new(),
        period: say!(
            text,
            assessment_period,
            date = written.format.date_with_year(date(day - 1)?)
        )?,
        headline: translations.shown(&assessment.headline).text.into(),
        sections,
    })
}

#[cfg(test)]
mod market_source_tests {
    use super::*;
    use crate::monitoring::business::{Fact, FactKind, Kind, Research};

    #[test]
    fn a_user_reads_only_distinct_verified_source_hosts_in_market_context() {
        let research = Research {
            day: 1,
            website: None,
            found: true,
            kind: Kind::Other,
            summary: String::new(),
            offers: vec![],
            areas: vec![],
            facts: [
                ("https://example.com/path?q=secret", true),
                ("https://example.com/other", true),
                ("https://other.example/market", true),
                ("https://unverified.example", false),
                ("not a url", true),
                ("mailto:editor@example.com", true),
            ]
            .into_iter()
            .map(|(source, verified)| Fact {
                claim: "Seasonal demand".into(),
                kind: FactKind::Seasonality,
                source: source.into(),
                verified,
            })
            .collect(),
        };
        assert_eq!(
            market_sources(Some(&research)),
            vec!["example.com", "other.example"]
        );
        assert!(market_sources(None).is_empty());
    }
}
