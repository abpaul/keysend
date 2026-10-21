use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::language::english;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum Window {
    SevenDays,
    ThirtyDays,
}

impl Window {
    pub fn days(self) -> i64 {
        match self {
            Self::SevenDays => 7,
            Self::ThirtyDays => 30,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Metrics {
    pub spend: f64,
    pub clicks: f64,
    pub impressions: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversions: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversion_value: Option<f64>,
}

impl Metrics {
    pub fn add(&mut self, other: &Self) {
        self.spend += other.spend;
        self.clicks += other.clicks;
        self.impressions += other.impressions;
        if let Some(value) = other.conversions {
            *self.conversions.get_or_insert(0.0) += value;
        }
        if let Some(value) = other.conversion_value {
            *self.conversion_value.get_or_insert(0.0) += value;
        }
    }

    pub fn without_conversions(mut self) -> Self {
        self.conversions = None;
        self.conversion_value = None;
        self
    }

    pub fn serving(&self) -> bool {
        self.impressions > 0.0 || self.clicks > 0.0 || self.spend > 0.0
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct WindowMetrics {
    pub window: Window,
    pub end_day: i64,
    pub current: Metrics,
    pub previous: Metrics,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct CampaignDay {
    pub day: i64,
    pub campaign_id: i64,
    pub metrics: Metrics,
}

/// Google Ads can still add conversions to a day after it has ended. An event that judges
/// conversions looks only at days that are at least this many days old.
pub const CONVERSION_MIN_AGE_DAYS: i64 = 2;

pub fn last_settled_conversion_day(end: i64) -> i64 {
    end - (CONVERSION_MIN_AGE_DAYS - 1)
}

pub fn last_shown_day(last_collected: i64, today: i64) -> i64 {
    last_collected.min(today - 1)
}

pub fn by_campaign(days: &[CampaignDay]) -> BTreeMap<i64, Vec<CampaignDay>> {
    let mut grouped: BTreeMap<i64, Vec<CampaignDay>> = BTreeMap::new();
    for row in days {
        grouped
            .entry(row.campaign_id)
            .or_default()
            .push(row.clone());
    }
    grouped
}

pub fn first_active_day(days: &[CampaignDay]) -> Option<i64> {
    days.iter()
        .filter(|row| {
            row.metrics.serving() || row.metrics.conversions.is_some_and(|value| value > 0.0)
        })
        .map(|row| row.day)
        .min()
}

pub fn sum(days: &[CampaignDay], start: i64, end: i64, campaign: Option<i64>) -> Metrics {
    let mut result = Metrics {
        conversions: Some(0.0),
        conversion_value: Some(0.0),
        ..Default::default()
    };
    for row in days.iter().filter(|row| {
        row.day >= start && row.day <= end && campaign.is_none_or(|id| row.campaign_id == id)
    }) {
        result.add(&row.metrics);
    }
    result
}

pub fn window_metrics(
    days: &[CampaignDay],
    end_day: i64,
    window: Window,
    campaign: Option<i64>,
    judge: bool,
) -> WindowMetrics {
    let length = window.days();
    let current = sum(days, end_day - length + 1, end_day, campaign);
    let previous = sum(days, end_day - length * 2 + 1, end_day - length, campaign);
    WindowMetrics {
        window,
        end_day,
        current: if judge {
            current
        } else {
            current.without_conversions()
        },
        previous: if judge {
            previous
        } else {
            previous.without_conversions()
        },
    }
}

pub(crate) fn ratio(numerator: f64, denominator: f64) -> Option<f64> {
    (denominator > 0.0).then(|| numerator / denominator)
}

pub fn english_number(n: f64, precision: usize) -> String {
    english().written.format.number(n, precision)
}

pub fn english_currency_prefix(currency: &str) -> String {
    if currency == "USD" {
        english().written.format.usd_symbol.into()
    } else {
        format!("{currency}{}", english().written.format.code_gap)
    }
}

pub fn english_money(value: f64, currency: &str) -> String {
    english().written.format.money(value, currency)
}

pub fn english_count(value: f64) -> String {
    english().written.format.count(value)
}

pub fn english_change(current: f64, previous: f64) -> String {
    english()
        .change(current, previous)
        .expect("the English change texts have their placeholders")
}

pub fn formatted(metrics: &Metrics, currency: &str) -> serde_json::Value {
    let mut facts = serde_json::json!({"Spend":english_money(metrics.spend,currency),"Clicks":english_count(metrics.clicks),"Impressions":english_count(metrics.impressions)});
    if let Some(value) = metrics.conversions {
        facts["Conversions"] = english_count(value).into();
    }
    if let Some(value) = metrics.conversion_value {
        facts["Conversion value"] = english_money(value, currency).into();
    }
    facts
}
