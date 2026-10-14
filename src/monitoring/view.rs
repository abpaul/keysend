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

pub fn campaign_cells(text: &Text, window: &WindowMetrics, currency: &str) -> Result<Vec<Cell>> {
    columns(window.current.conversions.is_some())[..3]
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

#[derive(Debug, Serialize, PartialEq)]
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
