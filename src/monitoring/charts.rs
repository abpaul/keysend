use std::fmt::Write as _;

use loco_rs::{Error, Result};
use serde::Serialize;

use super::{
    campaign_settings::{Target, TargetFrom},
    detection::{
        weekday_range, weekday_values, BASELINE_WEEKS, MIN_CLICK_CHANGE, MIN_CONVERSION_CHANGE,
        MIN_SPEND_CHANGE,
    },
    metrics::{sum, CampaignDay, Metrics},
};
use crate::{
    language::{english, fill, say, say_plural, Plural, Text, Written},
    workspace::history::date_of,
};

mod action_weeks;
mod deep;
mod search_share;
mod standalone;
pub use action_weeks::action_weeks;
pub use deep::{
    delay_sentence, demographic, hour_grid, meter, quality_sentence, score_bar, Group, LOW_SCORE,
    MIN_GRID_COUNT, MIN_KNOWN_SHARE,
};
pub use search_share::{search_share, SearchShare};
pub use standalone::{standalone, Frame};

pub const TREND_DAYS: i64 = BASELINE_WEEKS * 7;
pub const HISTORY_DAYS: i64 = TREND_DAYS * 2;
pub const WEEKS: i64 = 12;
pub const SPARK_DAYS: i64 = 30;
pub const SHARE_ROWS: usize = 8;
const SHARE_GAP: f64 = 5.0;

const WIDE: f64 = 600.0;
const PRINT_PLOT: f64 = WIDE;
const NARROW: f64 = 286.0;
const WEEKS_WIDE: f64 = WIDE + 10.0;
const WEEKS_NARROW: f64 = NARROW + 10.0;
const CHAR: f64 = 7.2;
const TIP_CHAR: f64 = 7.0;

const TREND_HEIGHT: f64 = 150.0;
const WEEKS_HEIGHT: f64 = 130.0;
const SLOT: f64 = 100.0 / WEEKS as f64;
const BAR_INSET: f64 = 0.66;
const BAR_WIDTH: f64 = 7.0;
const STEPS: [f64; 9] = [1.0, 1.5, 2.0, 2.5, 3.0, 4.0, 5.0, 6.0, 8.0];

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct Sentence {
    pub strong: String,
    pub text: String,
    pub mark: String,
    pub after: String,
}

impl Sentence {
    fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Default::default()
        }
    }

    pub fn words(&self) -> String {
        let lead = if self.strong.is_empty() { "" } else { " " };
        format!(
            "{}{lead}{}{}{}",
            self.strong, self.text, self.mark, self.after
        )
    }
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Key {
    pub kind: &'static str,
    pub label: String,
}

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct Table {
    pub caption: String,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub english_last: Vec<bool>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct ShareRow {
    pub name: String,
    pub href: Option<String>,
    pub svg: String,
    pub bars: Option<ShareBars>,
    pub values: Option<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ShareBars {
    pub spend: String,
    pub spend_width: f64,
    pub results: String,
    pub results_width: f64,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Chart {
    pub title: String,
    pub sentence: Sentence,
    pub svg: Option<String>,
    pub rows: Vec<ShareRow>,
    pub keys: Vec<Key>,
    pub table: Option<Table>,
}

impl Chart {
    pub fn empty(title: String, sentence: impl Into<String>) -> Self {
        Self {
            title,
            sentence: Sentence::plain(sentence),
            svg: None,
            rows: Vec::new(),
            keys: Vec::new(),
            table: None,
        }
    }
}

fn esc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            c => out.push(c),
        }
    }
    out
}

fn chars(text: &str) -> f64 {
    text.chars().count() as f64
}

fn chart_date(day: i64) -> Result<chrono::NaiveDate> {
    date_of(day).ok_or_else(|| Error::Message("Invalid chart day".into()))
}

fn short(text: &Text, day: i64) -> Result<String> {
    Ok(text.written.format.short_date(chart_date(day)?))
}

fn weekday(text: &Text, day: i64) -> Result<String> {
    Ok(text.written.format.weekday_short_date(chart_date(day)?))
}

fn tip(x: f64, y: f64, lines: &[String], to_left: bool) -> String {
    let width = lines.iter().map(|line| chars(line)).fold(0.0, f64::max) * TIP_CHAR + 16.0;
    let height = lines.len() as f64 * 16.0 + 10.0;
    let left = if to_left { x - width } else { x };
    let mut out = format!(
        r#"<g class="tip" aria-hidden="true"><rect x="{left:.1}" y="{y:.1}" width="{width:.1}" height="{height:.1}" rx="6"/>"#
    );
    for (index, line) in lines.iter().enumerate() {
        let _ = write!(
            out,
            r#"<text x="{:.1}" y="{:.1}">{}</text>"#,
            left + 8.0,
            y + 17.0 + index as f64 * 16.0,
            esc(line)
        );
    }
    out.push_str("</g>");
    out
}

fn nice_step(raw: f64) -> f64 {
    if raw <= 0.0 {
        return 1.0;
    }
    let power = 10f64.powf(raw.log10().floor());
    STEPS
        .iter()
        .map(|step| step * power)
        .find(|step| *step >= raw * (1.0 - 1e-9))
        .unwrap_or(10.0 * power)
}

fn line_scale(min: f64, max: f64) -> (f64, f64) {
    let mut step = nice_step(((max - min) / 2.0).max(max.abs() * 0.05));
    loop {
        let unit = 10f64.powf(step.log10().floor());
        let low = (min / unit).floor() * unit;
        if low + 2.0 * step >= max {
            return (low, step);
        }
        step = nice_step(step * 1.01);
    }
}

fn share(part: f64, total: f64) -> f64 {
    if total > 0.0 {
        part / total * 100.0
    } else {
        0.0
    }
}

fn share_text(text: &Text, percent: f64) -> String {
    let format = &text.written.format;
    if percent > 0.0 && percent < 0.5 {
        format!("<1{}", format.percent_suffix)
    } else {
        format!("{}{}", format.number(percent, 0), format.percent_suffix)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subject {
    Campaign,
    Device,
    Network,
    Location,
    AgeGroup,
    Gender,
}

struct SubjectWords<'a> {
    noun: &'a str,
    no_spend: &'a str,
    only_one: &'a str,
    no_results: &'a str,
    more: &'a Plural,
}

impl Subject {
    fn words(self, written: &Written) -> SubjectWords<'_> {
        match self {
            Self::Campaign => SubjectWords {
                noun: written.shares_campaign_noun,
                no_spend: written.shares_campaign_no_spend,
                only_one: written.shares_campaign_only_one,
                no_results: written.shares_campaign_no_results,
                more: &written.shares_campaign_more,
            },
            Self::Device => SubjectWords {
                noun: written.shares_device_noun,
                no_spend: written.shares_device_no_spend,
                only_one: written.shares_device_only_one,
                no_results: written.shares_device_no_results,
                more: &written.shares_device_more,
            },
            Self::Network => SubjectWords {
                noun: written.shares_network_noun,
                no_spend: written.shares_network_no_spend,
                only_one: written.shares_network_only_one,
                no_results: written.shares_network_no_results,
                more: &written.shares_network_more,
            },
            Self::Location => SubjectWords {
                noun: written.shares_location_noun,
                no_spend: written.shares_location_no_spend,
                only_one: written.shares_location_only_one,
                no_results: written.shares_location_no_results,
                more: &written.shares_location_more,
            },
            Self::AgeGroup => SubjectWords {
                noun: written.shares_age_noun,
                no_spend: written.shares_age_no_spend,
                only_one: written.shares_age_only_one,
                no_results: written.shares_age_no_results,
                more: &written.shares_age_more,
            },
            Self::Gender => SubjectWords {
                noun: written.shares_gender_noun,
                no_spend: written.shares_gender_no_spend,
                only_one: written.shares_gender_only_one,
                no_results: written.shares_gender_no_results,
                more: &written.shares_gender_more,
            },
        }
    }
}

struct Results<'a> {
    word: &'a str,
    title: &'a str,
}

fn results_of(written: &Written, judged: bool) -> Results<'_> {
    if judged {
        Results {
            word: written.results_conversions,
            title: written.metric_conversions,
        }
    } else {
        Results {
            word: written.results_clicks,
            title: written.metric_clicks,
        }
    }
}

pub fn shares_title(
    text: &Text,
    subject: Subject,
    judged: bool,
    period: Option<String>,
) -> Result<String> {
    let results = results_of(&text.written, judged).word;
    let noun = subject.words(&text.written).noun;
    match period {
        Some(period) => say!(
            text,
            shares_title,
            results = results,
            subject = noun,
            period = period
        ),
        None => say!(text, shares_title_bare, results = results, subject = noun),
    }
}

pub fn days(text: &Text, days: i64) -> Result<String> {
    say_plural!(text, period_days, u64::try_from(days).unwrap_or_default())
}

pub fn last_days(text: &Text, days: i64) -> Result<String> {
    say_plural!(
        text,
        period_last_days,
        u64::try_from(days).unwrap_or_default()
    )
}

pub struct Part {
    pub name: String,
    pub href: Option<String>,
    pub spend: f64,
    pub results: f64,
}

fn share_bar(
    kind: &str,
    y: f64,
    percent: f64,
    width: f64,
    text: &Text,
    title: &str,
    line: String,
) -> String {
    format!(
        r#"<g class="pt" tabindex="0" role="img"><title>{}</title><rect class="pt__hit" x="0" y="{y:.0}" width="100%" height="13" rx="4"/><rect class="bar bar--{kind}" x="0" y="{y:.0}" width="{width:.2}%" height="13" rx="4"/><text x="{width:.2}%" dx="6" y="{:.0}">{}</text>{}</g>"#,
        esc(title),
        y + 11.0,
        esc(&share_text(text, percent)),
        tip(0.0, y - 30.0, &[line], false),
    )
}

pub fn shares(
    text: &Text,
    subject: Subject,
    title: &str,
    parts: Vec<Part>,
    judged: bool,
    currency: &str,
) -> Result<Chart> {
    shares_of(
        text, subject, title, parts, None, judged, currency, SHARE_ROWS,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn shares_of(
    text: &Text,
    subject: Subject,
    title: &str,
    mut parts: Vec<Part>,
    rest: Option<Part>,
    judged: bool,
    currency: &str,
    rows: usize,
) -> Result<Chart> {
    let words = subject.words(&text.written);
    let results = results_of(&text.written, judged);
    let percent = |value: f64| share_text(text, value);
    parts.retain(|part| part.spend > 0.0 || part.results > 0.0);
    parts.sort_by(|a, b| b.spend.total_cmp(&a.spend).then(a.name.cmp(&b.name)));
    let rest = rest.filter(|rest| !parts.is_empty() && (rest.spend > 0.0 || rest.results > 0.0));
    let spend = parts
        .iter()
        .chain(&rest)
        .map(|part| part.spend)
        .sum::<f64>();
    let total = parts
        .iter()
        .chain(&rest)
        .map(|part| part.results)
        .sum::<f64>();
    if spend <= 0.0 {
        return Ok(Chart::empty(
            title.into(),
            fill("no_spend", words.no_spend, &[] as &[(&str, String)])?,
        ));
    }
    if let ([only], None) = (parts.as_slice(), &rest) {
        return Ok(Chart::empty(
            title.into(),
            fill("only_one", words.only_one, &[("name", only.name.as_str())])?,
        ));
    }
    let gap = |part: &Part| share(part.spend, spend) - share(part.results, total);
    let named = parts
        .iter()
        .max_by(|a, b| gap(a).total_cmp(&gap(b)))
        .filter(|part| total > 0.0 && gap(part) >= SHARE_GAP)
        .or(parts.first());
    let sentence = match named {
        None => Sentence::default(),
        Some(part) => {
            let spend_share = percent(share(part.spend, spend));
            if total > 0.0 {
                Sentence {
                    strong: part.name.clone(),
                    text: text.written.shares_takes.into(),
                    mark: say!(
                        text,
                        shares_takes_both,
                        spend = spend_share,
                        results = percent(share(part.results, total)),
                        kind = results.word
                    )?,
                    after: String::new(),
                }
            } else {
                Sentence {
                    strong: part.name.clone(),
                    text: text.written.shares_takes.into(),
                    mark: say!(text, shares_takes_spend, spend = spend_share)?,
                    after: format!(
                        " {}",
                        fill("no_results", words.no_results, &[("kind", results.word)])?
                    ),
                }
            }
        }
    };
    let table = Table {
        caption: title.into(),
        headers: vec![
            words.noun.into(),
            text.written.shares_header_spend.into(),
            text.written.share_of_spend.into(),
            results.title.into(),
            say!(text, share_of_results, results = results.word)?,
        ],
        rows: parts
            .iter()
            .chain(&rest)
            .map(|part| {
                vec![
                    part.name.clone(),
                    text.money(part.spend, currency),
                    percent(share(part.spend, spend)),
                    text.count(part.results),
                    percent(share(part.results, total)),
                ]
            })
            .collect(),
        ..Default::default()
    };
    let named_rows = rows - usize::from(rest.is_some());
    if parts.len() > named_rows {
        let more = parts.split_off(named_rows - 1);
        parts.push(Part {
            name: text.plural(words.more, more.len() as u64, &[])?,
            href: None,
            spend: more.iter().map(|part| part.spend).sum(),
            results: more.iter().map(|part| part.results).sum(),
        });
    }
    parts.extend(rest);
    let largest = parts
        .iter()
        .flat_map(|part| [share(part.spend, spend), share(part.results, total)])
        .fold(0.0, f64::max);
    let rows = parts
        .iter()
        .map(|part| {
            let spend_share = share(part.spend, spend);
            let result_share = share(part.results, total);
            let width = |percent: f64| {
                if largest > 0.0 {
                    percent / largest * 100.0
                } else {
                    0.0
                }
            };
            Ok(ShareRow {
                name: part.name.clone(),
                href: part.href.clone(),
                values: None,
                bars: Some(ShareBars {
                    spend: percent(spend_share),
                    spend_width: width(spend_share),
                    results: percent(result_share),
                    results_width: width(result_share),
                }),
                svg: format!(
                    r#"<svg class="chart__bars" height="29" role="group" aria-label="{}">{}{}</svg>"#,
                    esc(&part.name),
                    share_bar(
                        "spend",
                        0.0,
                        spend_share,
                        width(spend_share),
                        text,
                        &say!(
                            text,
                            shares_bar_spend_title,
                            name = &part.name,
                            share = percent(spend_share),
                            amount = text.money(part.spend, currency),
                            total = text.money(spend, currency)
                        )?,
                        say!(
                            text,
                            shares_bar_spend_tip,
                            amount = text.money(part.spend, currency),
                            total = text.money(spend, currency)
                        )?,
                    ),
                    share_bar(
                        "results",
                        16.0,
                        result_share,
                        width(result_share),
                        text,
                        &say!(
                            text,
                            shares_bar_results_title,
                            name = &part.name,
                            share = percent(result_share),
                            kind = results.word,
                            amount = text.count(part.results),
                            total = text.count(total)
                        )?,
                        say!(
                            text,
                            shares_bar_results_tip,
                            kind = results.title,
                            amount = text.count(part.results),
                            total = text.count(total)
                        )?,
                    ),
                ),
            })
        })
        .collect::<Result<_>>()?;
    Ok(Chart {
        title: title.into(),
        sentence,
        svg: None,
        rows,
        keys: vec![
            Key {
                kind: "spend",
                label: text.written.share_of_spend.into(),
            },
            Key {
                kind: "results",
                label: say!(text, share_of_results, results = results.word)?,
            },
        ],
        table: Some(table),
    })
}

const MIN_COST_PER_CLICK_CHANGE: f64 = 0.1;
const MIN_IMPRESSION_CHANGE: f64 = 100.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrendMetric {
    Spend,
    Clicks,
    Conversions,
    CostPerConversion,
    CostPerClick,
    Impressions,
}

impl TrendMetric {
    pub fn offered(judged: bool) -> &'static [Self] {
        if judged {
            &[
                Self::Spend,
                Self::Conversions,
                Self::CostPerConversion,
                Self::Clicks,
            ]
        } else {
            &[
                Self::Spend,
                Self::Clicks,
                Self::CostPerClick,
                Self::Impressions,
            ]
        }
    }

    pub fn chosen(key: Option<&str>, judged: bool) -> Self {
        Self::offered(judged)
            .iter()
            .copied()
            .find(|metric| Some(metric.key()) == key)
            .unwrap_or(Self::Spend)
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Spend => "spend",
            Self::Clicks => "clicks",
            Self::Conversions => "conversions",
            Self::CostPerConversion => "cost-per-conversion",
            Self::CostPerClick => "cost-per-click",
            Self::Impressions => "impressions",
        }
    }

    pub fn label(self, text: &Text) -> &'static str {
        let written = &text.written;
        match self {
            Self::Spend => written.metric_spend,
            Self::Clicks => written.metric_clicks,
            Self::Conversions => written.metric_conversions,
            Self::CostPerConversion => written.metric_cost_per_conversion,
            Self::CostPerClick => written.metric_cost_per_click,
            Self::Impressions => written.metric_impressions,
        }
    }

    pub fn per_day(self, text: &Text) -> &'static str {
        let written = &text.written;
        match self {
            Self::Spend => written.metric_per_day_spend,
            Self::Clicks => written.metric_per_day_clicks,
            Self::Conversions => written.metric_per_day_conversions,
            Self::CostPerConversion => written.metric_per_day_cost_per_conversion,
            Self::CostPerClick => written.metric_per_day_cost_per_click,
            Self::Impressions => written.metric_per_day_impressions,
        }
    }

    pub fn lower(self, text: &Text) -> &'static str {
        let written = &text.written;
        match self {
            Self::Spend => written.metric_spend_lower,
            Self::Clicks => written.metric_clicks_lower,
            Self::Conversions => written.metric_conversions_lower,
            Self::CostPerConversion => written.metric_cost_per_conversion_lower,
            Self::CostPerClick => written.metric_cost_per_click_lower,
            Self::Impressions => written.metric_impressions_lower,
        }
    }

    fn plural(self) -> bool {
        matches!(self, Self::Clicks | Self::Conversions | Self::Impressions)
    }

    fn is_money(self) -> bool {
        matches!(
            self,
            Self::Spend | Self::CostPerConversion | Self::CostPerClick
        )
    }

    fn value(self) -> fn(&Metrics) -> f64 {
        match self {
            Self::Spend => |metrics| metrics.spend,
            Self::Clicks => |metrics| metrics.clicks,
            Self::Conversions => |metrics| metrics.conversions.unwrap_or_default(),
            Self::CostPerConversion => |metrics| match metrics.conversions {
                Some(conversions) if conversions > 0.0 => metrics.spend / conversions,
                _ => f64::NAN,
            },
            Self::CostPerClick => |metrics| {
                if metrics.clicks > 0.0 {
                    metrics.spend / metrics.clicks
                } else {
                    f64::NAN
                }
            },
            Self::Impressions => |metrics| metrics.impressions,
        }
    }

    fn floor(self) -> f64 {
        match self {
            Self::Spend | Self::CostPerConversion => MIN_SPEND_CHANGE,
            Self::Clicks => MIN_CLICK_CHANGE,
            Self::Conversions => MIN_CONVERSION_CHANGE,
            Self::CostPerClick => MIN_COST_PER_CLICK_CHANGE,
            Self::Impressions => MIN_IMPRESSION_CHANGE,
        }
    }

    fn format(self, text: &Text, value: f64, currency: &str) -> String {
        if self.is_money() {
            text.money(value, currency)
        } else {
            text.count(value)
        }
    }

    fn axis(self, text: &Text, value: f64, currency: &str) -> String {
        axis(text, self.format(text, value, currency))
    }
}

fn axis(text: &Text, shown: String) -> String {
    let zeros = format!("{}00", text.written.format.decimal);
    shown
        .strip_suffix(zeros.as_str())
        .map(str::to_owned)
        .unwrap_or(shown)
}

#[derive(Clone, Debug, PartialEq)]
pub struct TrendDay {
    pub day: i64,
    pub value: Option<f64>,
    pub range: Option<(f64, f64)>,
    pub unusual: bool,
}

pub fn trend_days(
    days: &[CampaignDay],
    end: i64,
    first_day: i64,
    campaign: Option<i64>,
    metric: TrendMetric,
) -> Vec<TrendDay> {
    let ranged = end - first_day + 1 >= TREND_DAYS;
    ((end - TREND_DAYS + 1).max(first_day)..=end)
        .map(|day| {
            let value = metric.value()(&sum(days, day, day, campaign));
            let value = value.is_finite().then_some(value);
            let range = ranged
                .then(|| {
                    let usual = weekday_values(days, day, first_day, campaign, metric.value())
                        .into_iter()
                        .filter(|value| value.is_finite())
                        .collect::<Vec<_>>();
                    weekday_range(&usual, metric.floor())
                })
                .flatten();
            TrendDay {
                day,
                value,
                range: range.map(|range| range.usual()),
                unusual: value
                    .zip(range)
                    .is_some_and(|(value, range)| range.flags(value)),
            }
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub day: i64,
    pub text: String,
    pub label: Option<String>,
    pub english: bool,
}

impl Change {
    pub fn new(day: i64, text: impl Into<String>) -> Self {
        Self {
            day,
            text: text.into(),
            label: None,
            english: false,
        }
    }
}

pub fn change_label(text: &str) -> String {
    let Some((what, values)) = text.split_once(" changed from ") else {
        return text.into();
    };
    let Some((old, new)) = values.rsplit_once(" to ") else {
        return text.into();
    };
    let english = english();
    format!(
        "{what} {} → {}",
        axis(english, old.into()),
        axis(english, new.into())
    )
}

struct Mark {
    index: usize,
    day: i64,
    text: String,
    label: Option<String>,
    english: bool,
    more: usize,
}

fn trend_sentence(
    text: &Text,
    metric: TrendMetric,
    days: &[TrendDay],
    newest_change: Option<i64>,
) -> Result<String> {
    let name = metric.label(text);
    let ranged = days.iter().filter(|day| day.range.is_some()).count();
    if ranged == 0 {
        return say_plural!(
            text,
            trend_no_range,
            days.len() as u64,
            name = name,
            weeks = BASELINE_WEEKS
        );
    }
    let unusual = days.iter().filter(|day| day.unusual).collect::<Vec<_>>();
    let Some(last) = unusual.last() else {
        return say_plural!(text, trend_no_unusual, ranged as u64, name = name);
    };
    if let Some(change) = newest_change {
        let after = days.iter().filter(|day| day.day > change).count();
        let after_unusual = unusual.iter().filter(|day| day.day > change).count();
        if after_unusual > 0 {
            let date = short(text, change)?;
            return match (after == 1, metric.plural()) {
                (true, false) => say!(text, trend_after_day_was, name = name, date = date),
                (true, true) => say!(text, trend_after_day_were, name = name, date = date),
                (false, plural) => {
                    let forms = if plural {
                        &text.written.trend_after_days_were
                    } else {
                        &text.written.trend_after_days_was
                    };
                    text.plural(
                        forms,
                        after as u64,
                        &[
                            ("name", name),
                            ("unusual", &after_unusual.to_string()),
                            ("date", &date),
                        ],
                    )
                }
            };
        }
    }
    let forms = if metric.plural() {
        &text.written.trend_last_were
    } else {
        &text.written.trend_last_was
    };
    text.plural(
        forms,
        ranged as u64,
        &[
            ("name", name),
            ("unusual", &unusual.len().to_string()),
            ("date", &short(text, last.day)?),
        ],
    )
}

const LABEL_GAP: f64 = 6.0;
const LINE_CLEAR: f64 = 3.0;

#[derive(Debug, PartialEq)]
struct ChangeLabel {
    text: String,
    english: bool,
    at: f64,
    anchor: &'static str,
    dx: &'static str,
}

fn change_labels(
    text: &Text,
    marks: &[Mark],
    lines: &[f64],
    plot: f64,
    long: bool,
) -> Result<Vec<ChangeLabel>> {
    let at = |index: usize| lines[index] / 100.0 * plot;
    let dates = marks
        .iter()
        .map(|mark| short(text, mark.day))
        .collect::<Result<Vec<_>>>()?;
    let widest = dates.iter().map(|date| chars(date)).fold(0.0, f64::max) * CHAR;
    let mut groups: Vec<(usize, usize)> = Vec::new();
    for index in 0..marks.len() {
        match groups.last_mut() {
            Some(group) if at(index) - at(group.1) < widest + 2.0 * LABEL_GAP => group.1 = index,
            _ => groups.push((index, index)),
        }
    }
    let mut placed: Vec<(f64, f64)> = Vec::new();
    let mut labels = Vec::new();
    while let Some((first, last)) = groups.pop() {
        let mut texts = Vec::new();
        if first == last {
            if long && last + 1 == marks.len() {
                let label = marks[last]
                    .label
                    .clone()
                    .unwrap_or_else(|| change_label(&marks[last].text));
                texts.push((
                    match marks[last].more {
                        0 => say!(text, trend_label_one, label = label, date = &dates[last])?,
                        more => say!(
                            text,
                            trend_label_more,
                            label = label,
                            more = more,
                            date = &dates[last]
                        )?,
                    },
                    false,
                    marks[last].english,
                ));
            }
            texts.push((dates[last].clone(), true, false));
        } else {
            let range = say!(text, trend_dates, from = &dates[first], to = &dates[last])?;
            if long {
                let count = marks[first..=last]
                    .iter()
                    .map(|mark| mark.more + 1)
                    .sum::<usize>();
                texts.push((
                    say!(text, trend_label_group, count = count, range = &range)?,
                    false,
                    false,
                ));
            }
            texts.push((range, false, false));
        }
        let free = |from: f64, to: f64| {
            from >= 0.0
                && to <= plot
                && !(0..marks.len()).any(|index| {
                    !(first..=last).contains(&index)
                        && at(index) > from - LINE_CLEAR
                        && at(index) < to + LINE_CLEAR
                })
                && !placed
                    .iter()
                    .any(|(start, end)| from < end + LABEL_GAP && to > start - LABEL_GAP)
        };
        let found = texts.into_iter().find_map(|(label, centred, english)| {
            let width = chars(&label) * CHAR;
            let (left, right) = (at(first), at(last));
            let before = (
                left - LABEL_GAP - width,
                left - LABEL_GAP,
                first,
                "end",
                "-6",
            );
            let after = (
                right + LABEL_GAP,
                right + LABEL_GAP + width,
                last,
                "start",
                "6",
            );
            let middle = (left - width / 2.0, left + width / 2.0, first, "middle", "");
            let options = if centred {
                vec![middle, after, before]
            } else {
                vec![before, after]
            };
            options
                .into_iter()
                .find(|(from, to, ..)| free(*from, *to))
                .map(|(from, to, line, anchor, dx)| {
                    (
                        (from, to),
                        ChangeLabel {
                            text: label,
                            english,
                            at: lines[line],
                            anchor,
                            dx,
                        },
                    )
                })
        });
        match (found, groups.pop()) {
            (Some((space, label)), older) => {
                placed.push(space);
                labels.push(label);
                groups.extend(older);
            }
            (None, Some((older, _))) => groups.push((older, last)),
            (None, None) => {}
        }
    }
    Ok(labels)
}

pub fn trend(
    text: &Text,
    metric: TrendMetric,
    days: &[TrendDay],
    changes: &[Change],
    currency: &str,
) -> Result<Chart> {
    trend_of(text, metric, days, changes, currency, None)
}

pub fn trend_for_print(
    text: &Text,
    metric: TrendMetric,
    days: &[TrendDay],
    changes: &[Change],
    currency: &str,
) -> Result<Chart> {
    trend_of(text, metric, days, changes, currency, Some(PRINT_PLOT))
}

fn trend_of(
    text: &Text,
    metric: TrendMetric,
    days: &[TrendDay],
    changes: &[Change],
    currency: &str,
    plot_width: Option<f64>,
) -> Result<Chart> {
    let name = metric.label(text);
    let ranged = days.iter().any(|day| day.range.is_some());
    let title = if ranged {
        say!(text, trend_title_range, name = name)?
    } else {
        say!(text, trend_title, name = name)?
    };
    let values = days.iter().filter_map(|day| day.value).collect::<Vec<_>>();
    if days.len() < 2 || values.is_empty() {
        return Ok(Chart::empty(
            title,
            if days.len() < 2 {
                say!(text, trend_empty_short, name = name)?
            } else {
                say_plural!(
                    text,
                    trend_empty_none,
                    days.len() as u64,
                    name = metric.lower(text)
                )?
            },
        ));
    }
    let last = days.len() - 1;
    let x = |index: usize| index as f64 / last as f64 * 100.0;
    let (low, high) = days
        .iter()
        .flat_map(|day| {
            day.value
                .into_iter()
                .chain(day.range.into_iter().flat_map(|(low, high)| [low, high]))
        })
        .fold((f64::MAX, f64::MIN), |(low, high), value| {
            (low.min(value), high.max(value))
        });
    let (base, step) = line_scale(low, high);
    let y = |value: f64| TREND_HEIGHT - (value - base) / (2.0 * step) * TREND_HEIGHT;

    let mut marks: Vec<Mark> = Vec::new();
    for change in changes {
        let Some(index) = days.iter().position(|day| day.day == change.day) else {
            continue;
        };
        match marks.iter_mut().find(|mark| mark.day == change.day) {
            Some(mark) => mark.more += 1,
            None => marks.push(Mark {
                index,
                day: change.day,
                text: change.text.clone(),
                label: change.label.clone(),
                english: change.english,
                more: 0,
            }),
        }
    }
    marks.sort_by_key(|mark| mark.day);

    let mut svg = format!(
        r#"<svg class="chart__svg" height="190" role="group" aria-label="{}">"#,
        esc(&title)
    );
    let plot = |svg: &mut String, body: &str| {
        if plot_width.is_some() {
            svg.push_str(body);
        } else {
            let _ = write!(
                svg,
                r#"<svg width="100%" height="{TREND_HEIGHT:.0}" viewBox="0 0 {last} {TREND_HEIGHT:.0}" preserveAspectRatio="none">{body}</svg>"#
            );
        }
    };
    let across = |index: usize| match plot_width {
        Some(width) => format!("{:.1}", index as f64 / last as f64 * width),
        None => index.to_string(),
    };
    let mut band = String::new();
    let mut start = 0;
    while start < days.len() {
        if days[start].range.is_none() {
            start += 1;
            continue;
        }
        let run = days[start..]
            .iter()
            .take_while(|day| day.range.is_some())
            .count();
        let ranges = days[start..start + run]
            .iter()
            .filter_map(|day| day.range)
            .enumerate()
            .collect::<Vec<_>>();
        let mut path = String::new();
        for (offset, (_, high)) in &ranges {
            let _ = write!(
                path,
                "{}{},{:.1}",
                if *offset == 0 { "M" } else { "L" },
                across(start + offset),
                y(*high)
            );
        }
        for (offset, (low, _)) in ranges.iter().rev() {
            let _ = write!(path, "L{},{:.1}", across(start + offset), y(*low));
        }
        let _ = write!(band, r#"<path class="band" d="{path}Z"/>"#);
        start += run;
    }
    if !band.is_empty() {
        plot(&mut svg, &band);
    }
    for tick in 0..3 {
        let value = base + step * f64::from(tick);
        let _ = write!(
            svg,
            r#"<line class="grid" x1="0" x2="100%" y1="{0:.1}" y2="{0:.1}"/><text x="-30" y="{1:.1}">{2}</text>"#,
            y(value),
            y(value) + 4.0,
            esc(&metric.axis(text, value, currency))
        );
    }

    let lines = marks.iter().map(|mark| x(mark.index)).collect::<Vec<_>>();
    for at in &lines {
        let _ = write!(
            svg,
            r#"<line class="change" x1="{at:.2}%" x2="{at:.2}%" y1="0" y2="{TREND_HEIGHT:.0}"/>"#
        );
    }
    let newest = marks.last().map(|mark| mark.day);
    let wide = change_labels(text, &marks, &lines, plot_width.unwrap_or(WIDE), true)?;
    let narrow = change_labels(text, &marks, &lines, NARROW, false)?;
    let mut labels = String::new();
    let widths = wide
        .iter()
        .map(|label| (label, width_class(true, narrow.contains(label))))
        .chain(
            narrow
                .iter()
                .filter(|label| !wide.contains(label))
                .map(|label| (label, width_class(false, true))),
        );
    for (label, class) in widths {
        let dx = if label.dx.is_empty() {
            String::new()
        } else {
            format!(r#" dx="{}""#, label.dx)
        };
        let lang = if label.english { r#" lang="en""# } else { "" };
        let _ = write!(
            labels,
            r#"<text class="chart__change{}" x="{:.2}%"{dx} y="10" text-anchor="{}"{lang}>{}</text>"#,
            class.unwrap_or_default(),
            label.at,
            label.anchor,
            esc(&label.text)
        );
    }
    let mut line = String::new();
    let mut lone = vec![false; days.len()];
    let mut index = 0;
    while index < days.len() {
        if days[index].value.is_none() {
            index += 1;
            continue;
        }
        let run = days[index..]
            .iter()
            .take_while(|day| day.value.is_some())
            .count();
        if run == 1 {
            lone[index] = true;
        }
        let mut path = String::new();
        for (offset, value) in days[index..index + run]
            .iter()
            .filter_map(|day| day.value)
            .enumerate()
        {
            let _ = write!(
                path,
                "{}{},{:.1}",
                if offset == 0 { "M" } else { "L" },
                across(index + offset),
                y(value)
            );
        }
        let _ = write!(
            line,
            r#"<path class="line" d="{path}" vector-effect="non-scaling-stroke"/>"#
        );
        index += run;
    }
    plot(&mut svg, &line);
    svg.push_str(&labels);

    let mut rows = Vec::with_capacity(days.len());
    let mut english_last = Vec::with_capacity(days.len());
    for (index, day) in days.iter().enumerate() {
        let date = weekday(text, day.day)?;
        let value = match day.value {
            None => say!(text, trend_value_none, name = metric.lower(text))?,
            Some(value) => metric.format(text, value, currency),
        };
        let usual = day
            .range
            .map(|(low, high)| {
                say!(
                    text,
                    trend_range,
                    low = metric.format(text, low, currency),
                    high = metric.format(text, high, currency)
                )
            })
            .transpose()?;
        let day_line = say!(text, trend_tip_day, date = &date, value = &value)?;
        let mut lines = vec![day_line.clone()];
        let mut title = format!("{day_line}.");
        if let Some(usual) = &usual {
            let usual_line = say!(text, trend_tip_usual, usual = usual)?;
            let _ = write!(title, " {usual_line}.");
            lines.push(usual_line);
        }
        if day.unusual {
            let _ = write!(title, " {}.", text.written.trend_unusual_day);
        }
        let center = day.value.map_or(TREND_HEIGHT / 2.0, y);
        let height = lines.len() as f64 * 16.0 + 10.0;
        let top = if center - height - 10.0 < 0.0 {
            center + 10.0
        } else {
            center - height - 10.0
        };
        let to_left = x(index) > 50.0;
        let dot = match day.value {
            Some(_) if day.unusual => {
                format!(r#"<circle class="pt__dot pt__dot--out" cx="0" cy="{center:.1}" r="4"/>"#)
            }
            Some(_) if lone[index] => {
                format!(r#"<circle class="pt__dot pt__dot--lone" cx="0" cy="{center:.1}" r="2"/>"#)
            }
            Some(_) => format!(r#"<circle class="pt__dot" cx="0" cy="{center:.1}" r="4"/>"#),
            None => String::new(),
        };
        let _ = write!(
            svg,
            r#"<svg x="{:.2}%" width="1" height="1" overflow="visible"><g class="pt" tabindex="0" role="img"><title>{}</title><rect class="pt__hit" x="-5.5" y="0" width="11" height="{TREND_HEIGHT:.0}"/>{dot}{}</g></svg>"#,
            x(index),
            esc(&title),
            tip(if to_left { -8.0 } else { 8.0 }, top, &lines, to_left),
        );
        let on_day_changes = changes
            .iter()
            .filter(|change| change.day == day.day)
            .collect::<Vec<_>>();
        english_last.push(on_day_changes.iter().any(|change| change.english));
        let on_day = on_day_changes
            .iter()
            .map(|change| change.text.as_str())
            .collect::<Vec<_>>()
            .join(". ");
        rows.push(vec![
            date,
            if day.unusual {
                say!(text, trend_value_unusual, value = &value)?
            } else {
                value
            },
            usual.unwrap_or_else(|| text.written.not_available.into()),
            on_day,
        ]);
    }

    let mut ticks = (0..last).step_by(14).collect::<Vec<_>>();
    if ticks.len() > 1 && ticks.last().is_some_and(|tick| last - tick < 7) {
        ticks.pop();
    }
    ticks.push(last);
    let inner = ticks.len().saturating_sub(1);
    for (index, tick) in ticks.into_iter().enumerate() {
        let _ = write!(
            svg,
            r#"<text{} x="{:.2}%" y="{:.0}" text-anchor="{}">{}</text>"#,
            if index % 2 == 1 && index < inner {
                r#" class="tick--second""#
            } else {
                ""
            },
            x(tick),
            TREND_HEIGHT + 20.0,
            if tick == 0 {
                "start"
            } else if tick == last {
                "end"
            } else {
                "middle"
            },
            esc(&short(text, days[tick].day)?)
        );
    }
    svg.push_str("</svg>");

    let mut keys = vec![Key {
        kind: "line",
        label: name.into(),
    }];
    if ranged {
        keys.push(Key {
            kind: "band",
            label: text.written.trend_key_range.into(),
        });
        keys.push(Key {
            kind: "outside",
            label: text.written.trend_key_outside.into(),
        });
    }
    if !marks.is_empty() {
        keys.push(Key {
            kind: "change",
            label: text.written.trend_key_change.into(),
        });
    }
    Ok(Chart {
        sentence: Sentence::plain(trend_sentence(text, metric, days, newest)?),
        svg: Some(svg),
        rows: Vec::new(),
        keys,
        table: Some(Table {
            caption: say!(
                text,
                trend_caption,
                name = name,
                from = short(text, days[0].day)?,
                to = short(text, days[last].day)?
            )?,
            headers: vec![
                text.written.trend_header_day.into(),
                name.into(),
                text.written.trend_header_usual.into(),
                text.written.trend_header_change.into(),
            ],
            rows,
            english_last: if english_last.contains(&true) {
                english_last
            } else {
                Vec::new()
            },
        }),
        title,
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct Week {
    pub start: i64,
    pub spend: f64,
    pub conversions: f64,
}

impl Week {
    fn cost(&self) -> Option<f64> {
        (self.conversions > 0.0).then(|| self.spend / self.conversions)
    }
}

pub fn weeks(days: &[CampaignDay], end: i64, first_day: i64) -> Vec<Week> {
    (0..WEEKS)
        .rev()
        .map(|week| end - week * 7 - 6)
        .filter(|start| *start >= first_day)
        .map(|start| {
            let total = sum(days, start, start + 6, None);
            Week {
                start,
                spend: total.spend,
                conversions: total.conversions.unwrap_or_default(),
            }
        })
        .collect()
}

fn width_class(wide: bool, narrow: bool) -> Option<&'static str> {
    match (wide, narrow) {
        (true, true) => Some(""),
        (true, false) => Some(" wide"),
        (false, true) => Some(" narrow"),
        (false, false) => None,
    }
}

fn in_zone(center: f64, label: f64, zone: f64, plot: f64) -> bool {
    center / 100.0 * plot + label / 2.0 > plot - zone
}

const VALUE_ABOVE: f64 = -5.0;
const VALUE_INSIDE: f64 = 14.0;
const VALUE_HEIGHT: f64 = 12.0;
const VALUE_DESCENT: f64 = 5.0;
const TARGET_LIFT: f64 = -5.0;

pub fn cost_weeks(
    text: &Text,
    weeks: &[Week],
    target: Option<f64>,
    currency: &str,
) -> Result<Chart> {
    cost_weeks_for(
        text,
        weeks,
        target.map(|cost| Target {
            cost,
            from: TargetFrom::AdAccount,
        }),
        currency,
    )
}

pub fn cost_weeks_for(
    text: &Text,
    weeks: &[Week],
    target: Option<Target>,
    currency: &str,
) -> Result<Chart> {
    let from = target.map(|target| target.from);
    let target = target.map(|target| target.cost);
    let title = say_plural!(text, weeks_title, weeks.len().max(1) as u64)?;
    let costs = weeks.iter().map(Week::cost).collect::<Vec<_>>();
    let Some(highest) = costs.iter().flatten().copied().reduce(f64::max) else {
        return Ok(Chart::empty(
            say_plural!(text, weeks_title, WEEKS as u64)?,
            say_plural!(text, weeks_empty, WEEKS as u64)?,
        ));
    };
    let step = nice_step(highest.max(target.unwrap_or_default()) * 1.15 / 2.0);
    let y = |value: f64| WEEKS_HEIGHT - value / (2.0 * step) * WEEKS_HEIGHT;
    let newest = weeks.len() - 1;
    let slot = |index: usize| (WEEKS as usize - weeks.len() + index) as f64 * SLOT;
    let center = |index: usize| slot(index) + BAR_INSET + BAR_WIDTH / 2.0;
    let label = |value: f64| {
        axis(
            text,
            text.money(if value >= 10.0 { value.round() } else { value }, currency),
        )
    };

    let mut svg = format!(
        r#"<svg class="chart__svg" height="170" role="group" aria-label="{}">"#,
        esc(&title)
    );
    for tick in 1..=2 {
        let value = step * f64::from(tick);
        let _ = write!(
            svg,
            r#"<line class="grid" x1="0" x2="100%" y1="{0:.1}" y2="{0:.1}"/>"#,
            y(value)
        );
        if target.is_none_or(|target| (y(target) - y(value)).abs() >= 12.0) {
            let _ = write!(
                svg,
                r#"<text x="-30" y="{:.1}">{}</text>"#,
                y(value) + 4.0,
                esc(&label(value))
            );
        }
    }
    if let Some(target) = target {
        let _ = write!(
            svg,
            r#"<text x="-30" y="{:.1}">{}</text>"#,
            y(target) + 4.0,
            esc(&label(target))
        );
    }

    let widest = costs
        .iter()
        .flatten()
        .map(|cost| chars(&label(*cost)))
        .fold(0.0, f64::max)
        * CHAR;
    let stride = |plot: f64| ((widest + 4.0) / (plot / WEEKS as f64)).ceil().max(1.0) as usize;
    let target_text = target
        .map(|target| say!(text, weeks_target, value = label(target)))
        .transpose()?;
    let target_line = target.map(y);
    let zone = target_text
        .as_deref()
        .map_or(0.0, |text| chars(text) * CHAR + LABEL_GAP);
    let baseline = |index: usize, plot: f64| {
        let cost = costs[index]?;
        let top = y(cost);
        let width = chars(&label(cost)) * CHAR;
        let blocked = |at: f64| {
            target_line.is_some_and(|line| {
                (line > at - VALUE_HEIGHT && line < at + VALUE_DESCENT)
                    || (in_zone(center(index), width, zone, plot)
                        && (at - (line + TARGET_LIFT)).abs() < VALUE_HEIGHT + 2.0)
            })
        };
        let fits =
            width + 4.0 <= BAR_WIDTH / 100.0 * plot && WEEKS_HEIGHT - top >= VALUE_INSIDE + 6.0;
        if !blocked(top + VALUE_ABOVE) {
            Some((top + VALUE_ABOVE, false))
        } else if fits && !blocked(top + VALUE_INSIDE) {
            Some((top + VALUE_INSIDE, true))
        } else {
            None
        }
    };

    let mut bars = String::new();
    let mut values = String::new();
    let mut rows = Vec::with_capacity(weeks.len());
    for (index, week) in weeks.iter().enumerate() {
        let from = short(text, week.start)?;
        let open = index == newest;
        let totals = say!(
            text,
            weeks_totals,
            spend = text.money(week.spend, currency),
            conversions = text.count(week.conversions)
        )?;
        let (first_line, bar) = match costs[index] {
            Some(cost) => {
                let top = y(cost);
                let kind = if index == newest {
                    "bar--open"
                } else if target.is_some_and(|target| cost > target) {
                    "bar--above"
                } else {
                    "bar--spend"
                };
                let bar = format!(
                    r#"<rect class="bar {kind}" x="{:.2}%" y="{top:.1}" width="{BAR_WIDTH:.2}%" height="{:.1}" rx="4"/>"#,
                    slot(index) + BAR_INSET,
                    WEEKS_HEIGHT - top
                );
                let behind = newest - index;
                let shown = |plot: f64, inside: bool| {
                    Some(baseline(index, plot)?)
                        .filter(|(_, within)| {
                            *within == inside && behind.is_multiple_of(stride(plot))
                        })
                        .map(|(at, _)| at)
                };
                for inside in [false, true] {
                    let (wide, narrow) = (shown(WEEKS_WIDE, inside), shown(WEEKS_NARROW, inside));
                    let (Some(at), Some(class)) = (
                        wide.or(narrow),
                        width_class(wide.is_some(), narrow.is_some()),
                    ) else {
                        continue;
                    };
                    let _ = write!(
                        values,
                        r#"<text class="chart__value{}{class}" x="{:.2}%" y="{at:.1}" text-anchor="middle">{}</text>"#,
                        if inside && kind == "bar--spend" {
                            " chart__value--in"
                        } else {
                            ""
                        },
                        center(index),
                        esc(&label(cost))
                    );
                }
                let cost = text.money(cost, currency);
                (
                    if open {
                        say!(text, weeks_line_cost_open, from = &from, cost = cost)?
                    } else {
                        say!(text, weeks_line_cost, from = &from, cost = cost)?
                    },
                    bar,
                )
            }
            None => (
                if open {
                    say!(text, weeks_line_none_open, from = &from)?
                } else {
                    say!(text, weeks_line_none, from = &from)?
                },
                String::new(),
            ),
        };
        let to_left = center(index) > 50.0;
        let _ = write!(
            bars,
            r#"<g class="pt" tabindex="0" role="img"><title>{}</title><rect class="pt__hit" x="{:.2}%" y="0" width="{SLOT:.2}%" height="{WEEKS_HEIGHT:.0}"/>{bar}<svg x="{:.2}%" width="1" height="1" overflow="visible">{}</svg></g>"#,
            esc(&format!("{first_line}. {totals}.")),
            slot(index),
            center(index),
            tip(0.0, -44.0, &[first_line.clone(), totals], to_left),
        );
        rows.push(vec![
            if open {
                say!(text, weeks_row_open, from = &from)?
            } else {
                say!(text, weeks_row, from = &from)?
            },
            costs[index].map_or_else(
                || text.written.not_available.into(),
                |cost| text.money(cost, currency),
            ),
            text.money(week.spend, currency),
            text.count(week.conversions),
        ]);
    }
    svg.push_str(&bars);
    if let (Some(line), Some(text)) = (target_line, &target_text) {
        let _ = write!(
            svg,
            r#"<line class="target" x1="0" x2="100%" y1="{line:.1}" y2="{line:.1}"/>"#
        );
        let _ = write!(
            svg,
            r#"<text class="chart__target" x="100%" y="{:.1}" text-anchor="end">{}</text>"#,
            line + TARGET_LIFT,
            esc(text)
        );
    }
    svg.push_str(&values);
    let _ = write!(
        svg,
        r#"<text x="{:.2}%" y="{:.0}">{}</text>"#,
        slot(0) + BAR_INSET,
        WEEKS_HEIGHT + 18.0,
        esc(&short(text, weeks[0].start)?)
    );
    if newest > 1 {
        let _ = write!(
            svg,
            r#"<text x="100%" dx="-8" y="{:.0}" text-anchor="end">{}</text>"#,
            WEEKS_HEIGHT + 18.0,
            esc(&say!(
                text,
                weeks_axis_open,
                date = short(text, weeks[newest].start)?
            )?)
        );
    }
    svg.push_str("</svg>");

    let open = text.written.weeks_open;
    let sentence = match newest
        .checked_sub(1)
        .map(|index| (&weeks[index], costs[index]))
    {
        Some((week, Some(raw))) => {
            let (cost, date) = (text.money(raw, currency), short(text, week.start)?);
            let written = &text.written;
            let sentence = match (target, from) {
                (None, _) => fill(
                    "weeks_sentence",
                    written.weeks_sentence,
                    &[("cost", cost.as_str()), ("date", date.as_str())],
                ),
                (Some(target), from) => {
                    let campaign = matches!(from, Some(TargetFrom::Campaign));
                    let (key, template) = match (raw.partial_cmp(&target), campaign) {
                        (Some(std::cmp::Ordering::Greater), true) => (
                            "weeks_sentence_above_campaign",
                            written.weeks_sentence_above_campaign,
                        ),
                        (Some(std::cmp::Ordering::Less), true) => (
                            "weeks_sentence_below_campaign",
                            written.weeks_sentence_below_campaign,
                        ),
                        (_, true) => (
                            "weeks_sentence_at_campaign",
                            written.weeks_sentence_at_campaign,
                        ),
                        (Some(std::cmp::Ordering::Greater), false) => (
                            "weeks_sentence_above_account",
                            written.weeks_sentence_above_account,
                        ),
                        (Some(std::cmp::Ordering::Less), false) => (
                            "weeks_sentence_below_account",
                            written.weeks_sentence_below_account,
                        ),
                        (_, false) => (
                            "weeks_sentence_at_account",
                            written.weeks_sentence_at_account,
                        ),
                    };
                    fill(
                        key,
                        template,
                        &[
                            ("cost", cost.as_str()),
                            ("date", date.as_str()),
                            ("target", text.money(target, currency).as_str()),
                        ],
                    )
                }
            }?;
            format!("{sentence} {open}")
        }
        Some((week, None)) => {
            let none = say!(text, weeks_sentence_none, date = short(text, week.start)?)?;
            format!("{none} {open}")
        }
        None => open.into(),
    };
    let mut keys = Vec::new();
    if target.is_some() {
        keys.push(Key {
            kind: "spend",
            label: text.written.weeks_key_within.into(),
        });
        keys.push(Key {
            kind: "above",
            label: text.written.weeks_key_above.into(),
        });
    }
    keys.push(Key {
        kind: "open",
        label: text.written.weeks_key_open.into(),
    });
    Ok(Chart {
        title,
        sentence: Sentence::plain(sentence),
        svg: Some(svg),
        rows: Vec::new(),
        keys,
        table: Some(Table {
            caption: match target {
                Some(target) => say!(
                    text,
                    weeks_caption_target,
                    target = text.money(target, currency)
                )?,
                None => text.written.weeks_caption.into(),
            },
            headers: vec![
                text.written.weeks_header_week.into(),
                text.written.metric_cost_per_conversion.into(),
                text.written.metric_spend.into(),
                text.written.metric_conversions.into(),
            ],
            rows,
            ..Default::default()
        }),
    })
}

pub fn sparkline(values: &[f64], label: &str) -> String {
    let largest = values.iter().copied().fold(0.0, f64::max);
    let last = values.len().saturating_sub(1).max(1) as f64;
    let mut path = String::new();
    for (index, value) in values.iter().enumerate() {
        let height = if largest > 0.0 {
            value / largest * 26.0
        } else {
            0.0
        };
        let _ = write!(
            path,
            "{}{:.1},{:.1}",
            if index == 0 { "M" } else { "L" },
            index as f64 / last * 100.0,
            30.0 - height
        );
    }
    format!(
        r#"<svg class="spark" viewBox="0 0 100 32" width="100" height="28" role="img" aria-label="{0}"><title>{0}</title><path d="{path}"/></svg>"#,
        esc(label)
    )
}

pub fn metric_sparkline(values: &[Option<f64>], label: &str) -> String {
    let largest = values.iter().flatten().copied().fold(0.0, f64::max);
    let last = values.len().saturating_sub(1).max(1) as f64;
    let mut path = String::new();
    let mut drawing = false;
    for (index, value) in values.iter().enumerate() {
        let Some(value) = value else {
            drawing = false;
            continue;
        };
        let height = if largest > 0.0 {
            value / largest * 26.0
        } else {
            0.0
        };
        let _ = write!(
            path,
            "{}{:.1},{:.1}",
            if drawing { "L" } else { "M" },
            index as f64 / last * 100.0,
            30.0 - height
        );
        drawing = true;
    }
    format!(
        r#"<svg class="spark spark--metric" viewBox="0 0 100 32" preserveAspectRatio="none" role="img" aria-label="{0}"><title>{0}</title><path d="{path}"/></svg>"#,
        esc(label)
    )
}

#[cfg(test)]
pub(crate) mod english_calls {
    use loco_rs::Result;

    use super::{Change, Chart, Part, Subject, TrendDay, TrendMetric, Week};
    use crate::{language::english, monitoring::charts::deep::Group};

    fn subject(name: &str) -> Subject {
        match name {
            "Campaign" => Subject::Campaign,
            "device" => Subject::Device,
            "network" => Subject::Network,
            "location" => Subject::Location,
            "age group" => Subject::AgeGroup,
            "gender" => Subject::Gender,
            other => panic!("unknown subject {other}"),
        }
    }

    pub fn shares(
        title: &str,
        name: &str,
        parts: Vec<Part>,
        results: &str,
        currency: &str,
    ) -> Chart {
        super::shares(
            english(),
            subject(name),
            title,
            parts,
            results == "conversions",
            currency,
        )
        .unwrap()
    }

    pub fn shares_of(
        title: &str,
        name: &str,
        parts: Vec<Part>,
        rest: Option<Part>,
        results: &str,
        currency: &str,
        rows: usize,
    ) -> Chart {
        super::shares_of(
            english(),
            subject(name),
            title,
            parts,
            rest,
            results == "conversions",
            currency,
            rows,
        )
        .unwrap()
    }

    pub fn demographic(
        title: &str,
        name: &str,
        _what: &str,
        groups: Vec<Group>,
        results: &str,
        currency: &str,
    ) -> Chart {
        super::deep::demographic(
            english(),
            title,
            subject(name),
            groups,
            results == "conversions",
            currency,
        )
        .unwrap()
    }

    pub fn hour_grid(values: &[[f64; 24]; 7], results: &str) -> Chart {
        super::deep::hour_grid(english(), values, results == "conversions").unwrap()
    }

    pub fn search_share(title: &str, rows: Vec<super::SearchShare>) -> Chart {
        super::search_share(english(), title, rows).unwrap()
    }

    pub fn trend(
        metric: TrendMetric,
        days: &[TrendDay],
        changes: &[Change],
        currency: &str,
    ) -> Result<Chart> {
        super::trend(english(), metric, days, changes, currency)
    }

    pub fn trend_for_print(
        metric: TrendMetric,
        days: &[TrendDay],
        changes: &[Change],
        currency: &str,
    ) -> Result<Chart> {
        super::trend_for_print(english(), metric, days, changes, currency)
    }

    pub fn cost_weeks(weeks: &[Week], target: Option<f64>, currency: &str) -> Result<Chart> {
        super::cost_weeks(english(), weeks, target, currency)
    }
}

#[cfg(test)]
mod tests {
    use super::english_calls::{cost_weeks, shares, shares_of, trend, trend_for_print};
    use super::*;

    fn day(day: i64, campaign_id: i64, spend: f64, conversions: f64) -> CampaignDay {
        CampaignDay {
            day,
            campaign_id,
            metrics: Metrics {
                spend,
                clicks: spend / 2.0,
                impressions: spend * 10.0,
                conversions: Some(conversions),
                conversion_value: Some(0.0),
            },
        }
    }

    fn part(name: &str, spend: f64, results: f64) -> Part {
        Part {
            name: name.into(),
            href: Some(format!("/c/{name}")),
            spend,
            results,
        }
    }

    const END: i64 = 20_737;

    #[test]
    fn shares_name_the_part_with_the_largest_gap_and_scale_the_bars_to_the_largest_share() {
        let chart = shares(
            "Spend and conversions by Campaign",
            "Campaign",
            vec![
                part("Search - Brand", 410.0, 7.0),
                part("Search - Emergency", 1810.0, 61.0),
                part("Display - Remarketing", 780.0, 6.0),
                part("Search - Implants", 1120.0, 30.0),
            ],
            "conversions",
            "USD",
        );
        assert_eq!(
            chart.sentence.words(),
            "Display - Remarketing takes 19% of spend and brings 6% of conversions."
        );
        assert_eq!(chart.sentence.strong, "Display - Remarketing");
        assert_eq!(
            chart.sentence.mark,
            "19% of spend and brings 6% of conversions."
        );
        assert_eq!(
            chart
                .rows
                .iter()
                .map(|row| row.name.as_str())
                .collect::<Vec<_>>(),
            [
                "Search - Emergency",
                "Search - Implants",
                "Display - Remarketing",
                "Search - Brand"
            ]
        );
        let first = &chart.rows[0].svg;
        assert!(first.contains(
            r#"<rect class="bar bar--spend" x="0" y="0" width="74.90%" height="13" rx="4"/><text x="74.90%" dx="6" y="11">44%</text>"#
        ), "{first}");
        assert!(first.contains(
            r#"<rect class="bar bar--results" x="0" y="16" width="100.00%" height="13" rx="4"/><text x="100.00%" dx="6" y="27">59%</text>"#
        ));
        assert!(first.contains("<title>Search - Emergency: 44% of spend, $1,810 of $4,120</title>"));
        assert!(first.contains(r#"<text x="8.0" y="-13.0">Spend: $1,810 of $4,120</text>"#));
        assert!(first.contains(r#"<text x="8.0" y="3.0">Conversions: 61 of 104</text>"#));
        assert_eq!(
            chart.table.unwrap().rows[2],
            ["Display - Remarketing", "$780", "19%", "6", "6%"]
        );
        assert_eq!(chart.keys[1].label, "Share of conversions");
    }

    #[test]
    fn shares_without_results_or_without_spend_say_so_and_many_parts_fold_into_one_row() {
        let none = shares("By device", "device", vec![], "clicks", "USD");
        assert_eq!(none.sentence.words(), "No device had spend in these days.");
        assert!(none.rows.is_empty() && none.table.is_none() && none.keys.is_empty());

        let unconverted = shares(
            "By Campaign",
            "Campaign",
            vec![part("A", 30.0, 0.0), part("B", 70.0, 0.0)],
            "conversions",
            "USD",
        );
        assert_eq!(
            unconverted.sentence.words(),
            "B takes 70% of spend. No Campaign had conversions in these days."
        );
        assert!(unconverted.rows[0]
            .svg
            .contains(r#"<rect class="bar bar--results" x="0" y="16" width="0.00%""#));

        let even = shares(
            "By device",
            "device",
            vec![part("Tablet", 7.0, 6.0), part("Mobile", 93.0, 94.0)],
            "clicks",
            "USD",
        );
        assert_eq!(
            even.sentence.words(),
            "Mobile takes 93% of spend and brings 94% of clicks."
        );

        let many = shares(
            "By Campaign",
            "Campaign",
            (1..=10)
                .map(|index| part(&format!("C{index:02}"), f64::from(100 - index), 1.0))
                .collect(),
            "clicks",
            "USD",
        );
        assert_eq!(many.rows.len(), SHARE_ROWS);
        assert_eq!(many.rows[7].name, "3 more Campaigns");
        assert_eq!(many.rows[7].href, None);
        assert_eq!(many.table.unwrap().rows.len(), 10);
        assert_eq!(many.keys[1].label, "Share of clicks");
    }

    #[test]
    fn shares_of_one_row_are_one_sentence() {
        let one = shares(
            "Spend and conversions by device, last 7 days",
            "device",
            vec![part("Mobile", 4120.0, 104.0), part("Desktop", 0.0, 0.0)],
            "conversions",
            "USD",
        );
        assert_eq!(
            one,
            Chart {
                title: "Spend and conversions by device, last 7 days".into(),
                sentence: Sentence {
                    strong: String::new(),
                    text: "Only one device had spend in these days: Mobile.".into(),
                    mark: String::new(),
                    after: String::new(),
                },
                svg: None,
                rows: Vec::new(),
                keys: Vec::new(),
                table: None,
            }
        );

        let with_rest = shares_of(
            "Spend and clicks by location, 30 days",
            "location",
            vec![part("Harbour City", 60.0, 6.0)],
            Some(part("Other locations", 40.0, 4.0)),
            "clicks",
            "USD",
            6,
        );
        assert_eq!(
            with_rest
                .rows
                .iter()
                .map(|row| row.name.as_str())
                .collect::<Vec<_>>(),
            ["Harbour City", "Other locations"]
        );

        let results_elsewhere = shares(
            "By device",
            "device",
            vec![part("Mobile", 100.0, 0.0), part("Desktop", 0.0, 3.0)],
            "conversions",
            "USD",
        );
        assert_eq!(results_elsewhere.rows.len(), 2);
    }

    #[test]
    fn text_in_a_chart_is_escaped() {
        let chart = shares(
            "By Campaign",
            "Campaign",
            vec![
                part("<script>\"x\"</script> & co", 10.0, 1.0),
                part("Other", 5.0, 1.0),
            ],
            "clicks",
            "USD",
        );
        let svg = &chart.rows[0].svg;
        assert!(!svg.contains("<script>"));
        assert!(svg.contains(r#"aria-label="&lt;script&gt;&quot;x&quot;&lt;/script&gt; &amp; co""#));
        assert!(!sparkline(&[1.0], "\"><b>").contains("<b>"));
    }

    #[test]
    fn three_ticks_hold_the_values() {
        assert_eq!(line_scale(210.0, 790.0), (200.0, 300.0));
        assert_eq!(line_scale(0.0, 12.0), (0.0, 6.0));
        assert_eq!(line_scale(195.0, 790.0), (100.0, 400.0));
        assert_eq!(line_scale(40.0, 40.0), (40.0, 2.0));
        assert_eq!(line_scale(0.0, 0.0), (0.0, 1.0));
    }

    #[test]
    fn a_change_label_is_short() {
        assert_eq!(
            change_label("Daily budget changed from $45.00 to $20.00"),
            "Daily budget $45 → $20"
        );
        assert_eq!(
            change_label("Bid strategy changed from maximize clicks to maximize conversions"),
            "Bid strategy maximize clicks → maximize conversions"
        );
        assert_eq!(change_label("Campaign paused"), "Campaign paused");
    }

    fn steady(first: i64) -> Vec<CampaignDay> {
        (first..=END).map(|at| day(at, 1, 100.0, 4.0)).collect()
    }

    #[test]
    fn a_trend_day_has_the_usual_range_and_the_judgement_of_the_detection_rules() {
        let mut days = steady(END - 111);
        days.last_mut().unwrap().metrics.spend = 151.0;
        let trend = trend_days(&days, END, END - 111, None, TrendMetric::Spend);
        assert_eq!(trend.len(), 56);
        assert_eq!(trend[0].day, END - 55);
        assert_eq!(trend[0].range, Some((100.0, 100.0)));
        assert_eq!(trend[55].value, Some(151.0));
        assert!(trend[55].unusual);
        assert!(!trend[54].unusual);

        days.last_mut().unwrap().metrics.spend = 150.0;
        assert!(!trend_days(&days, END, END - 111, None, TrendMetric::Spend)[55].unusual);

        let mut varied = steady(END - 111);
        for (index, row) in varied.iter_mut().enumerate() {
            row.metrics.spend = if (index / 7) % 2 == 0 { 80.0 } else { 120.0 };
        }
        let band = trend_days(&varied, END, END - 111, None, TrendMetric::Spend);
        assert_eq!(band[55].range, Some((80.0, 120.0)));
        assert!(!band[55].unusual);

        let short_history = steady(END - 54);
        let thin = trend_days(&short_history, END, END - 54, None, TrendMetric::Spend);
        assert_eq!(thin.len(), 55);
        assert!(thin.iter().all(|day| day.range.is_none()));

        let eight_weeks = trend_days(&steady(END - 55), END, END - 55, None, TrendMetric::Spend);
        assert_eq!(eight_weeks[27].range, None);
        assert_eq!(eight_weeks[28].range, Some((100.0, 100.0)));

        let mut unconverted = steady(END - 111);
        unconverted.last_mut().unwrap().metrics.conversions = Some(0.0);
        let cost = trend_days(
            &unconverted,
            END,
            END - 111,
            None,
            TrendMetric::CostPerConversion,
        );
        assert_eq!(cost[54].value, Some(25.0));
        assert_eq!(cost[54].range, Some((25.0, 25.0)));
        assert_eq!(cost[55].value, None);
        assert!(!cost[55].unusual);
    }

    fn flat(count: usize, value: f64, range: Option<(f64, f64)>) -> Vec<TrendDay> {
        (0..count)
            .map(|index| TrendDay {
                day: END + 1 - count as i64 + index as i64,
                value: Some(value),
                range,
                unusual: false,
            })
            .collect()
    }

    #[test]
    fn the_trend_draws_the_band_the_line_the_unusual_days_and_the_account_change() {
        let mut days = flat(5, 500.0, Some((200.0, 800.0)));
        days[3].value = Some(100.0);
        days[4].value = Some(900.0);
        days[3].unusual = true;
        days[4].unusual = true;
        let chart = trend(
            TrendMetric::Spend,
            &days,
            &[Change::new(
                END - 2,
                "Daily budget changed from $45.00 to $20.00",
            )],
            "USD",
        )
        .unwrap();
        assert_eq!(chart.title, "Spend per day, against its usual range");
        assert_eq!(
            chart.sentence.words(),
            "Spend was unusual on 2 of the 2 days after the Account change of 9 Oct."
        );
        let svg = chart.svg.unwrap();
        assert!(svg.starts_with(
            r#"<svg class="chart__svg" height="190" role="group" aria-label="Spend per day, against its usual range">"#
        ));
        assert!(svg.contains(
            r#"<svg width="100%" height="150" viewBox="0 0 4 150" preserveAspectRatio="none"><path class="band" d="M0,18.8L1,18.8L2,18.8L3,18.8L4,18.8L4,131.2L3,131.2L2,131.2L1,131.2L0,131.2Z"/></svg>"#
        ), "{svg}");
        assert!(svg.contains(
            r#"<path class="line" d="M0,75.0L1,75.0L2,75.0L3,150.0L4,0.0" vector-effect="non-scaling-stroke"/>"#
        ));
        for tick in [
            r#"<line class="grid" x1="0" x2="100%" y1="150.0" y2="150.0"/><text x="-30" y="154.0">$100</text>"#,
            r#"<line class="grid" x1="0" x2="100%" y1="75.0" y2="75.0"/><text x="-30" y="79.0">$500</text>"#,
            r#"<line class="grid" x1="0" x2="100%" y1="0.0" y2="0.0"/><text x="-30" y="4.0">$900</text>"#,
        ] {
            assert!(svg.contains(tick), "{tick}");
        }
        assert!(svg.contains(r#"<line class="change" x1="50.00%" x2="50.00%" y1="0" y2="150"/>"#));
        assert!(svg.contains(
            r#"<text class="chart__change wide" x="50.00%" dx="-6" y="10" text-anchor="end">Daily budget $45 → $20 · 9 Oct</text>"#
        ));
        assert!(svg.contains(
            r#"<text class="chart__change narrow" x="50.00%" y="10" text-anchor="middle">9 Oct</text>"#
        ));
        assert_eq!(svg.matches(r#"class="pt" tabindex="0""#).count(), 5);
        assert_eq!(svg.matches("pt__dot--out").count(), 2);
        assert!(svg.contains(
            r#"<svg x="75.00%" width="1" height="1" overflow="visible"><g class="pt" tabindex="0" role="img"><title>Sat 10 Oct: $100. Usual: $200 to $800. Unusual day.</title><rect class="pt__hit" x="-5.5" y="0" width="11" height="150"/><circle class="pt__dot pt__dot--out" cx="0" cy="150.0" r="4"/>"#
        ));
        assert!(svg.contains(r#"<text x="0.00%" y="170" text-anchor="start">7 Oct</text>"#));
        assert!(svg.contains(r#"<text x="100.00%" y="170" text-anchor="end">11 Oct</text>"#));
        let table = chart.table.unwrap();
        assert_eq!(table.caption, "Spend per day, 7 Oct to 11 Oct");
        assert_eq!(
            table.rows[2],
            [
                "Fri 9 Oct",
                "$500",
                "$200 to $800",
                "Daily budget changed from $45.00 to $20.00"
            ]
        );
        assert_eq!(table.rows[3][1], "$100, unusual day");
        assert_eq!(
            chart.keys.iter().map(|key| key.kind).collect::<Vec<_>>(),
            ["line", "band", "outside", "change"]
        );
    }

    #[test]
    fn the_trend_sentence_follows_the_days() {
        let inside = flat(56, 500.0, Some((200.0, 800.0)));
        assert_eq!(
            trend(TrendMetric::Clicks, &inside, &[], "USD")
                .unwrap()
                .sentence
                .words(),
            "Clicks had no unusual day in the last 56 days."
        );

        let mut spike = inside.clone();
        spike[16].value = Some(900.0);
        spike[16].unusual = true;
        let chart = trend(
            TrendMetric::Clicks,
            &spike,
            &[Change::new(
                END - 20,
                "Daily budget changed from $45.00 to $20.00",
            )],
            "USD",
        )
        .unwrap();
        assert_eq!(
            chart.sentence.words(),
            "Clicks were unusual on 1 of the last 56 days, the last time on 2 Sep."
        );

        let thin = trend(TrendMetric::Spend, &flat(21, 500.0, None), &[], "USD").unwrap();
        assert_eq!(thin.title, "Spend per day");
        assert_eq!(
            thin.sentence.words(),
            "Spend per day for 21 days. The chart has no usual range, because the Account history has fewer than 8 weeks."
        );
        let svg = thin.svg.unwrap();
        assert!(!svg.contains("band") && !svg.contains("pt__dot--out"));
        assert_eq!(thin.keys.len(), 1);

        let one = trend(TrendMetric::Spend, &flat(1, 500.0, None), &[], "USD").unwrap();
        assert_eq!(one.svg, None);
        assert_eq!(
            one.sentence.words(),
            "Spend per day appears here after 2 days of Account history."
        );

        let mut none = flat(10, 0.0, None);
        for day in &mut none {
            day.value = None;
        }
        assert_eq!(
            trend(TrendMetric::CostPerConversion, &none, &[], "USD")
                .unwrap()
                .sentence
                .words(),
            "No day of the last 10 had a cost per conversion."
        );
    }

    fn change_texts(svg: &str) -> Vec<&str> {
        svg.split(r#"<text class="chart__change"#)
            .skip(1)
            .filter_map(|rest| rest.split("</text>").next())
            .collect()
    }

    fn budget_change(offset: i64) -> Change {
        Change::new(
            END - offset,
            "Display - Remarketing: Daily budget changed from $45.00 to $20.00",
        )
    }

    #[test]
    fn change_labels_do_not_collide() {
        let days = flat(56, 500.0, None);
        let svg = trend(
            TrendMetric::Spend,
            &days,
            &[
                budget_change(40),
                budget_change(13),
                budget_change(11),
                budget_change(11),
            ],
            "USD",
        )
        .unwrap()
        .svg
        .unwrap();
        assert_eq!(svg.matches(r#"<line class="change""#).count(), 3);
        assert_eq!(
            change_texts(&svg),
            [
                r#" wide" x="76.36%" dx="-6" y="10" text-anchor="end">3 Account changes · 28 Sep to 30 Sep"#,
                r#" wide" x="27.27%" y="10" text-anchor="middle">1 Sep"#,
                r#" narrow" x="76.36%" dx="-6" y="10" text-anchor="end">28 Sep to 30 Sep"#,
                r#" narrow" x="27.27%" dx="-6" y="10" text-anchor="end">1 Sep"#,
            ]
        );
    }

    #[test]
    fn a_change_label_does_not_cross_the_line_of_another_change() {
        let days = flat(56, 500.0, None);
        let near = [budget_change(16), budget_change(7), budget_change(3)];
        let svg = trend(TrendMetric::Spend, &days, &near, "USD")
            .unwrap()
            .svg
            .unwrap();
        for line in ["70.91%", "87.27%", "94.55%"] {
            assert!(svg.contains(&format!(r#"<line class="change" x1="{line}""#)));
        }
        assert_eq!(
            change_texts(&svg),
            [
                r#" wide" x="70.91%" dx="-6" y="10" text-anchor="end">3 Account changes · 25 Sep to 8 Oct"#,
                r#" narrow" x="70.91%" dx="-6" y="10" text-anchor="end">25 Sep to 8 Oct"#,
            ]
        );

        let print = trend_for_print(TrendMetric::Spend, &days, &near, "USD")
            .unwrap()
            .svg
            .unwrap();
        assert_eq!(change_texts(&print), change_texts(&svg));

        let apart = [budget_change(30), budget_change(3)];
        let svg = trend(TrendMetric::Spend, &days, &apart, "USD")
            .unwrap()
            .svg
            .unwrap();
        assert_eq!(
            change_texts(&svg),
            [
                r#" wide" x="94.55%" y="10" text-anchor="middle">8 Oct"#,
                r#"" x="45.45%" y="10" text-anchor="middle">11 Sep"#,
                r#" narrow" x="94.55%" dx="-6" y="10" text-anchor="end">8 Oct"#,
            ]
        );
    }

    #[test]
    fn a_change_label_stays_inside_the_plot() {
        let days = flat(56, 500.0, None);
        let labels = |offset: i64| {
            let svg = trend(TrendMetric::Spend, &days, &[budget_change(offset)], "USD")
                .unwrap()
                .svg
                .unwrap();
            change_texts(&svg)
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            labels(0),
            [
                r#" wide" x="100.00%" dx="-6" y="10" text-anchor="end">Display - Remarketing: Daily budget $45 → $20 · 11 Oct"#,
                r#" narrow" x="100.00%" dx="-6" y="10" text-anchor="end">11 Oct"#,
            ]
        );
        assert_eq!(
            labels(50),
            [
                r#" wide" x="9.09%" dx="6" y="10" text-anchor="start">Display - Remarketing: Daily budget $45 → $20 · 22 Aug"#,
                r#" narrow" x="9.09%" y="10" text-anchor="middle">22 Aug"#,
            ]
        );
        assert_eq!(
            labels(55),
            [
                r#" wide" x="0.00%" dx="6" y="10" text-anchor="start">Display - Remarketing: Daily budget $45 → $20 · 17 Aug"#,
                r#" narrow" x="0.00%" dx="6" y="10" text-anchor="start">17 Aug"#,
            ]
        );
    }

    #[test]
    fn weeks_are_seven_days_that_end_on_the_last_day() {
        let days = (END - 30..=END)
            .map(|at| day(at, 1, 10.0, 1.0))
            .collect::<Vec<_>>();
        let weeks = weeks(&days, END, END - 30);
        assert_eq!(weeks.len(), 4);
        assert_eq!(weeks[0].start, END - 27);
        assert_eq!(weeks[3].start, END - 6);
        assert_eq!(weeks[3].spend, 70.0);
        assert_eq!(weeks[3].conversions, 7.0);
    }

    fn mock_weeks() -> Vec<Week> {
        [58, 55, 57, 52, 49, 50, 46, 44, 47, 41, 40, 38]
            .iter()
            .enumerate()
            .map(|(index, cost)| Week {
                start: END - 6 - (11 - index as i64) * 7,
                spend: f64::from(*cost) * 20.0,
                conversions: 20.0,
            })
            .collect()
    }

    #[test]
    fn cost_weeks_draw_the_bars_against_the_target() {
        let chart = cost_weeks(&mock_weeks(), Some(45.0), "USD").unwrap();
        assert_eq!(chart.title, "Cost per conversion, 12 weeks");
        assert_eq!(
            chart.sentence.words(),
            "Cost per conversion was $40.00 in the week of 28 Sep, below the target from the Ad Account profile ($45.00). The last week is not complete: conversions can still arrive."
        );
        let svg = chart.svg.unwrap();
        assert!(svg.contains(
            r#"<rect class="bar bar--above" x="0.66%" y="35.8" width="7.00%" height="94.2" rx="4"/>"#
        ), "{svg}");
        assert!(svg.contains(
            r#"<text class="chart__value wide" x="4.16%" y="30.8" text-anchor="middle">$58</text>"#
        ));
        assert!(svg.contains(
            r#"<rect class="bar bar--spend" x="58.99%" y="58.5" width="7.00%" height="71.5" rx="4"/>"#
        ));
        assert!(svg.contains(
            r#"<rect class="bar bar--open" x="92.33%" y="68.2" width="7.00%" height="61.8" rx="4"/>"#
        ));
        assert!(svg.contains(
            r#"<text class="chart__value wide" x="95.83%" y="82.2" text-anchor="middle">$38</text>"#
        ));
        assert!(svg.contains(
            r#"<text class="chart__value" x="12.49%" y="35.6" text-anchor="middle">$55</text>"#
        ));
        assert!(svg.contains(
            r#"<text class="chart__value chart__value--in wide" x="87.49%" y="79.0" text-anchor="middle">$40</text>"#
        ));
        assert_eq!(svg.matches(">$40</text>").count(), 1);
        assert!(svg.contains(
            r#"<line class="target" x1="0" x2="100%" y1="56.9" y2="56.9"/><text class="chart__target" x="100%" y="51.9" text-anchor="end">Target $45</text>"#
        ));
        assert!(svg.contains(
            r#"<line class="grid" x1="0" x2="100%" y1="0.0" y2="0.0"/><text x="-30" y="4.0">$80</text>"#
        ));
        assert!(svg.contains(
            r#"<line class="grid" x1="0" x2="100%" y1="65.0" y2="65.0"/><line class="grid""#
        ));
        assert!(svg.contains(r#"<text x="-30" y="60.9">$45</text>"#));
        assert!(!svg.contains(r#"<text x="-30" y="69.0">"#));
        assert!(svg.contains(r#"<text x="0.66%" y="148">20 Jul</text>"#));
        assert!(svg
            .contains(r#"<text x="100%" dx="-8" y="148" text-anchor="end">5 Oct (so far)</text>"#));
        assert!(svg.contains(
            "<title>Week of 5 Oct (so far): $38.00. $760 spend, 20 conversions.</title>"
        ));
        assert_eq!(svg.matches(r#"class="pt" tabindex="0""#).count(), 12);
        let table = chart.table.unwrap();
        assert_eq!(
            table.caption,
            "Cost per conversion per week. The target is $45.00."
        );
        assert_eq!(table.rows[0], ["Week of 20 Jul", "$58.00", "$1,160", "20"]);
        assert_eq!(table.rows[11][0], "Week of 5 Oct (so far)");
    }

    fn value_labels(svg: &str) -> Vec<&str> {
        svg.split(r#"<text class="chart__value"#)
            .skip(1)
            .filter_map(|rest| rest.split("</text>").next())
            .collect()
    }

    #[test]
    fn a_value_label_that_the_target_line_would_strike_is_inside_its_bar() {
        let svg = cost_weeks(&mock_weeks(), Some(45.0), "USD")
            .unwrap()
            .svg
            .unwrap();
        assert!(svg.contains(r#"<line class="target" x1="0" x2="100%" y1="56.9" y2="56.9"/>"#));
        assert_eq!(
            value_labels(&svg),
            [
                r#" wide" x="4.16%" y="30.8" text-anchor="middle">$58"#,
                r#"" x="12.49%" y="35.6" text-anchor="middle">$55"#,
                r#" wide" x="20.83%" y="32.4" text-anchor="middle">$57"#,
                r#"" x="29.16%" y="40.5" text-anchor="middle">$52"#,
                r#" wide" x="37.49%" y="45.4" text-anchor="middle">$49"#,
                r#"" x="45.83%" y="43.8" text-anchor="middle">$50"#,
                r#" wide" x="54.16%" y="50.2" text-anchor="middle">$46"#,
                r#" chart__value--in wide" x="62.49%" y="72.5" text-anchor="middle">$44"#,
                r#" wide" x="70.83%" y="48.6" text-anchor="middle">$47"#,
                r#" chart__value--in wide" x="79.16%" y="77.4" text-anchor="middle">$41"#,
                r#" chart__value--in wide" x="87.49%" y="79.0" text-anchor="middle">$40"#,
                r#" wide" x="95.83%" y="82.2" text-anchor="middle">$38"#,
            ]
        );
    }

    #[test]
    fn the_right_end_is_for_the_target_label() {
        let mut weeks = mock_weeks();
        weeks[11].spend = 46.0 * 20.0;
        let svg = cost_weeks(&weeks, Some(45.0), "USD").unwrap().svg.unwrap();
        assert!(svg.contains(
            r#"<text class="chart__target" x="100%" y="51.9" text-anchor="end">Target $45</text>"#
        ));
        assert!(svg.contains(
            r#"<rect class="bar bar--open" x="92.33%" y="55.2" width="7.00%" height="74.8" rx="4"/>"#
        ));
        assert_eq!(
            value_labels(&svg)[10..],
            [
                r#" chart__value--in wide" x="87.49%" y="79.0" text-anchor="middle">$40"#,
                r#" wide" x="95.83%" y="69.2" text-anchor="middle">$46"#,
            ]
        );

        weeks[11].spend = 48.0 * 20.0;
        let struck_inside = cost_weeks(&weeks, Some(45.0), "USD").unwrap().svg.unwrap();
        assert!(struck_inside.contains(
            r#"<rect class="bar bar--open" x="92.33%" y="52.0" width="7.00%" height="78.0" rx="4"/>"#
        ));
        assert_eq!(
            value_labels(&struck_inside).last(),
            Some(&r#" chart__value--in wide" x="87.49%" y="79.0" text-anchor="middle">$40"#)
        );
    }

    #[test]
    fn cost_weeks_without_a_target_or_without_conversions() {
        let chart = cost_weeks(&mock_weeks(), None, "USD").unwrap();
        assert_eq!(
            chart.sentence.words(),
            "Cost per conversion was $40.00 in the week of 28 Sep. The last week is not complete: conversions can still arrive."
        );
        let svg = chart.svg.unwrap();
        assert!(!svg.contains("target") && !svg.contains("bar--above"));
        assert_eq!(chart.keys.len(), 1);

        let mut weeks = mock_weeks();
        weeks[10].conversions = 0.0;
        let gap = cost_weeks(&weeks, Some(45.0), "USD").unwrap();
        assert!(gap
            .sentence
            .words()
            .starts_with("The week of 28 Sep had no conversions. "));
        assert!(gap
            .svg
            .unwrap()
            .contains("<title>Week of 28 Sep: no conversions. $800 spend, 0 conversions.</title>"));
        assert_eq!(gap.table.unwrap().rows[10][1], "Not available");

        for week in &mut weeks {
            week.conversions = 0.0;
        }
        let none = cost_weeks(&weeks, Some(45.0), "USD").unwrap();
        assert_eq!(none.svg, None);
        assert_eq!(
            none.sentence.words(),
            "No week of the last 12 had conversions, so there is no cost per conversion to show."
        );
    }

    #[test]
    fn the_sparkline_runs_from_zero_to_the_largest_value() {
        assert_eq!(
            sparkline(&[0.0, 50.0, 100.0], "Spend per day"),
            r#"<svg class="spark" viewBox="0 0 100 32" width="100" height="28" role="img" aria-label="Spend per day"><title>Spend per day</title><path d="M0.0,30.0L50.0,17.0L100.0,4.0"/></svg>"#
        );
        assert!(sparkline(&[0.0, 0.0], "x").contains(r#"d="M0.0,30.0L100.0,30.0""#));
        assert_eq!(
            metric_sparkline(&[Some(50.0), None, Some(100.0), Some(0.0)], "Cost per day"),
            r#"<svg class="spark spark--metric" viewBox="0 0 100 32" preserveAspectRatio="none" role="img" aria-label="Cost per day"><title>Cost per day</title><path d="M0.0,17.0M66.7,4.0L100.0,30.0"/></svg>"#
        );
    }

    #[test]
    fn the_trend_offers_no_conversion_metric_without_judging_by_conversions() {
        assert_eq!(
            TrendMetric::chosen(Some("conversions"), true),
            TrendMetric::Conversions
        );
        assert_eq!(
            TrendMetric::chosen(Some("cost-per-conversion"), false),
            TrendMetric::Spend
        );
        assert_eq!(
            TrendMetric::chosen(Some("clicks"), false),
            TrendMetric::Clicks
        );
        assert_eq!(TrendMetric::chosen(None, true), TrendMetric::Spend);
    }
}
