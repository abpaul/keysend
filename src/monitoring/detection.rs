use serde::{Deserialize, Serialize};

use super::campaign_settings::Rules;
use super::causes::Candidate;
use super::metrics::{
    by_campaign, last_settled_conversion_day, sum, window_metrics, CampaignDay, Metrics, Window,
    WindowMetrics,
};
use crate::workspace::history::{AccountChange, Campaign};

pub const BASELINE_WEEKS: i64 = 8;
pub const MIN_BASELINE_WEEKS: usize = 3;
pub const WEEK_DEVIATIONS: f64 = 2.0;
pub const NEW_ACCOUNT_CHANGE: f64 = 0.5;
pub const MIN_SPEND_CHANGE: f64 = 1.0;
pub const MIN_CLICK_CHANGE: f64 = 10.0;
pub const MIN_CONVERSION_CHANGE: f64 = 3.0;
pub const MIN_NORMAL_DAYS: usize = 4;
pub const MIN_SERVING_SHARE: f64 = 0.5;
pub const DAY_DEVIATIONS: f64 = 3.0;

pub const ACCOUNT_CHANGED: &str = "Account change in the last 7 days";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum MetricName {
    Spend,
    Clicks,
    Conversions,
}

impl MetricName {
    pub const ALL: [Self; 3] = [Self::Spend, Self::Clicks, Self::Conversions];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Spend => "Spend",
            Self::Clicks => "Clicks",
            Self::Conversions => "Conversions",
        }
    }

    pub fn value(self, metrics: &Metrics) -> f64 {
        match self {
            Self::Spend => metrics.spend,
            Self::Clicks => metrics.clicks,
            Self::Conversions => metrics.conversions.unwrap_or_default(),
        }
    }

    pub(super) fn floor(self) -> f64 {
        match self {
            Self::Spend => MIN_SPEND_CHANGE,
            Self::Clicks => MIN_CLICK_CHANGE,
            Self::Conversions => MIN_CONVERSION_CHANGE,
        }
    }

    fn last_day(self, end: i64) -> i64 {
        match self {
            Self::Conversions => last_settled_conversion_day(end),
            Self::Spend | Self::Clicks => end,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Movement {
    pub metric: MetricName,
    pub current: f64,
    pub previous: f64,
    pub normal_variation: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct CampaignMovement {
    pub campaign_id: i64,
    pub name: String,
    pub metrics: WindowMetrics,
    pub movements: Vec<Movement>,
    pub reasons: Vec<String>,
    pub account_changes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub follow_ups: Vec<super::follow_up::Checked>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Detection {
    pub day: i64,
    pub currency: String,
    pub metrics: WindowMetrics,
    pub movements: Vec<Movement>,
    pub yesterday: Vec<String>,
    pub campaigns: Vec<CampaignMovement>,
    pub judge_by_conversions: bool,
    #[serde(default)]
    pub causes: Vec<Candidate>,
}

impl Detection {
    pub fn quiet(&self) -> bool {
        self.movements.is_empty() && self.yesterday.is_empty() && self.campaigns.is_empty()
    }
}

pub fn mean_deviation(values: &[f64]) -> (f64, f64) {
    if values.is_empty() {
        return (0.0, 0.0);
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let deviation = (values
        .iter()
        .map(|value| (value - mean).powi(2))
        .sum::<f64>()
        / values.len() as f64)
        .sqrt();
    (mean, deviation)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeekdayRange {
    pub mean: f64,
    pub deviation: f64,
    pub tolerance: f64,
}

impl WeekdayRange {
    pub fn usual(&self) -> (f64, f64) {
        (
            (self.mean - self.deviation).max(0.0),
            self.mean + self.deviation,
        )
    }

    pub fn flags(&self, value: f64) -> bool {
        (value - self.mean).abs() > self.tolerance
    }
}

pub fn weekday_range(values: &[f64], floor: f64) -> Option<WeekdayRange> {
    let (mean, deviation) = mean_deviation(values);
    (values.len() >= MIN_NORMAL_DAYS).then(|| WeekdayRange {
        mean,
        deviation,
        tolerance: (DAY_DEVIATIONS * deviation)
            .max(mean * NEW_ACCOUNT_CHANGE)
            .max(floor),
    })
}

pub fn normal_range(values: &[f64], floor: f64) -> Option<(f64, f64)> {
    weekday_range(values, floor).map(|range| (range.mean, range.tolerance))
}

pub fn weekday_values(
    days: &[CampaignDay],
    day: i64,
    first_day: i64,
    campaign: Option<i64>,
    metric: impl Fn(&Metrics) -> f64,
) -> Vec<f64> {
    (1..=BASELINE_WEEKS)
        .map(|week| day - week * 7)
        .filter(|day| *day >= first_day)
        .map(|day| metric(&sum(days, day, day, campaign)))
        .collect()
}

pub fn normal_serving(
    days: &[CampaignDay],
    day: i64,
    first_day: i64,
    campaign: Option<i64>,
) -> bool {
    let values = weekday_values(days, day, first_day, campaign, |metrics| {
        f64::from(metrics.serving())
    });
    values.len() >= MIN_NORMAL_DAYS
        && values.iter().filter(|value| **value > 0.0).count() >= MIN_NORMAL_DAYS
        && values.iter().sum::<f64>() / values.len() as f64 >= MIN_SERVING_SHARE
}

pub fn stopped_on(days: &[CampaignDay], day: i64, first: i64, campaign: Option<i64>) -> bool {
    if sum(days, day, day, campaign).serving() || !normal_serving(days, day, first, campaign) {
        return false;
    }
    let Some(last) = days
        .iter()
        .filter(|row| {
            row.day < day
                && campaign.is_none_or(|id| id == row.campaign_id)
                && row.metrics.serving()
        })
        .map(|row| row.day)
        .max()
    else {
        return false;
    };
    (last + 1..=day).find(|candidate| normal_serving(days, *candidate, first, campaign))
        == Some(day)
}

pub(super) fn lagged_window(days: &[CampaignDay], end: i64, judge: bool) -> WindowMetrics {
    let mut window = window_metrics(days, end, Window::SevenDays, None, judge);
    if judge {
        let settled = window_metrics(
            days,
            last_settled_conversion_day(end),
            Window::SevenDays,
            None,
            judge,
        );
        window.current.conversions = settled.current.conversions;
        window.current.conversion_value = settled.current.conversion_value;
        window.previous.conversions = settled.previous.conversions;
        window.previous.conversion_value = settled.previous.conversion_value;
    }
    window
}

fn weekly(days: &[CampaignDay], end: i64, first_day: i64, judge: bool) -> Vec<Movement> {
    let window = lagged_window(days, end, judge);
    MetricName::ALL
        .into_iter()
        .map(|name| (name, name.last_day(end)))
        .filter(|(name, field_end)| {
            (judge || *name != MetricName::Conversions) && first_day <= field_end - 13
        })
        .filter_map(|(name, field_end)| {
            let (value, floor) = (|metrics: &Metrics| name.value(metrics), name.floor());
            let weeks = (1..=BASELINE_WEEKS)
                .filter(|week| field_end - week * 7 - 6 >= first_day)
                .map(|week| sum(days, field_end - week * 7 - 6, field_end - week * 7, None))
                .collect::<Vec<_>>();
            let current = value(&window.current);
            let previous = value(&window.previous);
            let variation = if weeks.len() >= MIN_BASELINE_WEEKS {
                let changes = weeks
                    .windows(2)
                    .map(|pair| (value(&pair[0]) - value(&pair[1])).abs())
                    .collect::<Vec<_>>();
                let (mean, deviation) = mean_deviation(&changes);
                (mean + WEEK_DEVIATIONS * deviation).max(floor)
            } else {
                (previous * NEW_ACCOUNT_CHANGE).max(floor)
            };
            ((current - previous).abs() > variation).then_some(Movement {
                metric: name,
                current,
                previous,
                normal_variation: variation,
            })
        })
        .collect()
}

pub fn detect(
    days: &[CampaignDay],
    campaigns: &[Campaign],
    changes: &[AccountChange],
    end: i64,
    first_day: i64,
    rules: impl Rules,
    currency: &str,
) -> Detection {
    let judge = rules.judged();
    let mut detection = Detection {
        day: end + 1,
        currency: currency.into(),
        metrics: lagged_window(days, end, judge),
        movements: weekly(days, end, first_day, judge),
        yesterday: Vec::new(),
        campaigns: Vec::new(),
        judge_by_conversions: judge,
        causes: Vec::new(),
    };
    let names = campaigns
        .iter()
        .map(|campaign| (campaign.id, campaign.name.clone()))
        .collect();
    let yesterday = crate::workspace::history::date_of(end)
        .map(|date| date.to_string())
        .unwrap_or_default();
    for change in changes
        .iter()
        .filter(|change| change.changed_at.starts_with(&yesterday))
    {
        detection
            .yesterday
            .push(change_words(change, &names, currency));
    }
    for name in MetricName::ALL {
        if !judge && name == MetricName::Conversions {
            continue;
        }
        let day = name.last_day(end);
        let value = name.value(&sum(days, day, day, None));
        let values = weekday_values(days, day, first_day, None, |metrics| name.value(metrics));
        if let Some((mean, _)) = normal_range(&values, name.floor())
            .filter(|(mean, tolerance)| (value - mean).abs() > *tolerance)
        {
            let format = |value| {
                if name == MetricName::Spend {
                    super::metrics::english_money(value, currency)
                } else {
                    super::metrics::english_count(value)
                }
            };
            detection
                .yesterday
                .push(if name == MetricName::Conversions {
                    format!(
                        "On {} conversions were {}, against a usual {} for that weekday.",
                        crate::workspace::history::date_of(day)
                            .map(|date| date.format("%A %-d %B").to_string())
                            .unwrap_or_default(),
                        format(value),
                        format(mean)
                    )
                } else {
                    format!(
                        "Yesterday {} was {}, against a usual {} for that weekday.",
                        name.as_str().to_lowercase(),
                        format(value),
                        format(mean)
                    )
                });
        }
    }
    let grouped = by_campaign(days);
    for campaign in campaigns {
        let mut reasons = Vec::new();
        let own_days = grouped
            .get(&campaign.id)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let campaign_judge = rules.campaign_judged(campaign.id);
        let metrics = lagged_window(own_days, end, campaign_judge);
        let before = sum(own_days, first_day, end - 7, None);
        if metrics.current.serving() && !before.serving() {
            reasons.push("Campaign started in the last 7 days".into());
            if !sum(own_days, first_day, end - 1, None).serving()
                && sum(own_days, end, end, None).serving()
            {
                detection
                    .yesterday
                    .push(format!("{} started yesterday.", campaign.name));
            }
        }
        if let Some(stopped) =
            (end - 6..=end).find(|day| stopped_on(own_days, *day, first_day, None))
        {
            reasons.push(format!(
                "Campaign stopped serving on {}",
                crate::workspace::history::date_of(stopped)
                    .map(|date| date.format("%-d %B").to_string())
                    .unwrap_or_default()
            ));
            if stopped == end {
                detection
                    .yesterday
                    .push(format!("{} stopped serving yesterday.", campaign.name));
            }
        }
        let mut own_changes = changes
            .iter()
            .filter(|change| change.campaign_id == Some(campaign.id))
            .collect::<Vec<_>>();
        own_changes.sort_by(|a, b| {
            b.changed_at
                .cmp(&a.changed_at)
                .then(b.resource_name.cmp(&a.resource_name))
        });
        if !own_changes.is_empty() {
            reasons.push(ACCOUNT_CHANGED.into());
        }
        let movements = weekly(own_days, end, first_day, campaign_judge);
        if !movements.is_empty() || !reasons.is_empty() {
            detection.campaigns.push(CampaignMovement {
                campaign_id: campaign.id,
                name: campaign.name.clone(),
                metrics,
                movements,
                reasons,
                account_changes: own_changes
                    .into_iter()
                    .map(|change| change_words(change, &names, currency))
                    .collect(),
                follow_ups: Vec::new(),
            });
        }
    }
    detection
        .campaigns
        .sort_by_key(|campaign| campaign.campaign_id);
    detection
}

fn change_words(
    change: &AccountChange,
    names: &std::collections::BTreeMap<i64, String>,
    currency: &str,
) -> String {
    let day = |text: &str| {
        chrono::NaiveDate::parse_from_str(text.get(..10).unwrap_or_default(), "%Y-%m-%d")
            .map_or_else(
                |_| text.to_owned(),
                |date| date.format("%-d %B").to_string(),
            )
    };
    if let Some(readable) = crate::google_ads::change_text(change, names, currency) {
        let subject = if readable.campaign.is_empty() {
            "The Ad Account"
        } else {
            readable.campaign.as_str()
        };
        format!("{subject}: {} on {}", readable.text, day(&readable.date))
    } else {
        format!(
            "{}: a change to its {} on {}",
            names
                .get(&change.campaign_id.unwrap_or_default())
                .map(String::as_str)
                .unwrap_or("The Ad Account"),
            change.resource_type.to_lowercase().replace('_', " "),
            day(&change.changed_at)
        )
    }
}
