use std::collections::BTreeMap;

use loco_rs::prelude::*;
use serde_json::{json, Value};

use super::with_window;
use crate::{
    language::{say, Language, Text},
    monitoring::{
        campaign_settings::Target,
        charts::{self, Change, Chart, Part, Subject, TrendMetric},
        metrics::{CampaignDay, Window},
        view::{self, MetricView},
    },
    translation::{Translations, Wanted},
    workspace::{
        history::{day_number, AccountChange, Campaign},
        WorkspaceStore,
    },
};

pub(super) struct Source<'a> {
    pub text: &'a Text,
    pub id: &'a str,
    pub days: &'a [CampaignDay],
    pub from: i64,
    pub end: i64,
    pub first_day: i64,
    pub judged: bool,
    pub currency: &'a str,
    pub window: Window,
}

impl Source<'_> {
    pub fn results(&self) -> &'static str {
        if self.judged {
            self.text.written.results_conversions
        } else {
            self.text.written.results_clicks
        }
    }

    fn result_of(&self, day: &CampaignDay) -> f64 {
        if self.judged {
            day.metrics.conversions.unwrap_or_default()
        } else {
            day.metrics.clicks
        }
    }

    pub fn share_title(&self, subject: Subject) -> Result<String> {
        charts::shares_title(
            self.text,
            subject,
            self.judged,
            Some(charts::last_days(self.text, self.window.days())?),
        )
    }

    pub fn trend(
        &self,
        campaign: Option<i64>,
        metric: TrendMetric,
        changes: &[Change],
    ) -> Result<Chart> {
        charts::trend(
            self.text,
            metric,
            &charts::trend_days(self.days, self.end, self.first_day, campaign, metric),
            changes,
            self.currency,
        )
    }

    pub fn trend_for_print(
        &self,
        campaign: Option<i64>,
        metric: TrendMetric,
        changes: &[Change],
    ) -> Result<Chart> {
        charts::trend_for_print(
            self.text,
            metric,
            &charts::trend_days(self.days, self.end, self.first_day, campaign, metric),
            changes,
            self.currency,
        )
    }

    pub fn cost_weeks(&self, target: Option<f64>) -> Result<Chart> {
        charts::cost_weeks(
            self.text,
            &charts::weeks(self.days, self.end, self.first_day),
            target,
            self.currency,
        )
    }

    pub fn campaign_cost_weeks(&self, campaign: i64, target: Option<Target>) -> Result<Chart> {
        let own = self
            .days
            .iter()
            .filter(|day| day.campaign_id == campaign)
            .cloned()
            .collect::<Vec<_>>();
        charts::cost_weeks_for(
            self.text,
            &charts::weeks(&own, self.end, self.first_day),
            target,
            self.currency,
        )
    }

    pub fn campaign_shares(
        &self,
        campaigns: &[Campaign],
        href: impl Fn(i64) -> String,
    ) -> Result<Chart> {
        let start = self.end - self.window.days() + 1;
        let mut totals: BTreeMap<i64, (f64, f64)> = BTreeMap::new();
        for day in self.days.iter().filter(|day| day.day >= start) {
            let total = totals.entry(day.campaign_id).or_default();
            total.0 += day.metrics.spend;
            total.1 += self.result_of(day);
        }
        charts::shares(
            self.text,
            Subject::Campaign,
            &self.share_title(Subject::Campaign)?,
            campaigns
                .iter()
                .filter_map(|campaign| {
                    totals.get(&campaign.id).map(|(spend, results)| Part {
                        name: campaign.name.clone(),
                        href: Some(href(campaign.id)),
                        spend: *spend,
                        results: *results,
                    })
                })
                .collect(),
            self.judged,
            self.currency,
        )
    }

    pub async fn device_shares(
        &self,
        store: &WorkspaceStore<'_>,
        campaign: Option<i64>,
    ) -> Result<Chart> {
        let parts = store
            .device_totals(
                self.id,
                self.end - self.window.days() + 1,
                self.end,
                campaign,
            )
            .await?
            .into_iter()
            .map(|total| Part {
                name: match total.device {
                    1 => self.text.written.device_mobile,
                    2 => self.text.written.device_desktop,
                    3 => self.text.written.device_tablet,
                    4 => self.text.written.device_tv,
                    _ => self.text.written.device_other,
                }
                .into(),
                href: None,
                spend: total.spend,
                results: if self.judged {
                    total.conversions
                } else {
                    total.clicks
                },
            })
            .collect();
        charts::shares(
            self.text,
            Subject::Device,
            &self.share_title(Subject::Device)?,
            parts,
            self.judged,
            self.currency,
        )
    }

    pub fn sparklines(&self) -> Result<BTreeMap<i64, String>> {
        let start = (self.end - charts::SPARK_DAYS + 1).max(self.first_day);
        let length = usize::try_from(self.end - start + 1).unwrap_or_default();
        if length < 2 {
            return Ok(BTreeMap::new());
        }
        let mut spend: BTreeMap<i64, Vec<f64>> = BTreeMap::new();
        for day in self.days.iter().filter(|day| day.day >= start) {
            spend
                .entry(day.campaign_id)
                .or_insert_with(|| vec![0.0; length])[(day.day - start) as usize] +=
                day.metrics.spend;
        }
        let (from, to) = (
            view::short_date(self.text, start)?,
            view::short_date(self.text, self.end)?,
        );
        spend
            .into_iter()
            .map(|(campaign, values)| {
                let amount = |value: f64| self.text.money(value, self.currency);
                let label = say!(
                    self.text,
                    spark_spend,
                    from = &from,
                    to = &to,
                    first = amount(values[0]),
                    last = amount(values[values.len() - 1]),
                    highest = amount(values.iter().copied().fold(0.0, f64::max)),
                )?;
                Ok((campaign, charts::sparkline(&values, &label)))
            })
            .collect()
    }

    pub fn metric_sparklines(&self, campaign: Option<i64>) -> Result<Vec<Option<String>>> {
        let start = (self.end - self.window.days() + 1).max(self.first_day);
        let (from, to) = (
            view::short_date(self.text, start)?,
            view::short_date(self.text, self.end)?,
        );
        view::metric_days(self.days, start, self.end, campaign, self.judged)
            .into_iter()
            .map(|(column, values)| {
                let present = values.iter().flatten().copied();
                let (Some(lowest), Some(highest)) = (
                    present.clone().reduce(f64::min),
                    present.clone().reduce(f64::max),
                ) else {
                    return Ok(None);
                };
                if present.count() < 2 {
                    return Ok(None);
                }
                let label = say!(
                    self.text,
                    spark_metric,
                    metric = (column.label)(&self.text.written),
                    from = &from,
                    to = &to,
                    lowest = column.show(self.text, lowest, self.currency),
                    highest = column.show(self.text, highest, self.currency),
                )?;
                Ok(Some(charts::metric_sparkline(&values, &label)))
            })
            .collect()
    }

    fn trend_start(&self) -> i64 {
        (self.end - charts::TREND_DAYS + 1).max(self.first_day)
    }

    pub async fn stored_changes(
        &self,
        store: &WorkspaceStore<'_>,
        earlier_days: i64,
    ) -> Result<Vec<AccountChange>> {
        let date = |day: i64| {
            crate::workspace::history::date_of(day)
                .ok_or_else(|| Error::Message("Invalid chart day".into()))
        };
        store
            .account_changes(
                self.id,
                date(self.trend_start() - earlier_days)?,
                date(self.end)?,
            )
            .await
    }

    pub async fn raw_changes(
        &self,
        store: &WorkspaceStore<'_>,
        names: &BTreeMap<i64, String>,
        campaign: Option<i64>,
    ) -> Result<Vec<RawChange>> {
        self.readable_changes(&self.stored_changes(store, 0).await?, names, campaign)
    }

    pub fn trend_changes(
        &self,
        stored: &[AccountChange],
        names: &BTreeMap<i64, String>,
        campaign: Option<i64>,
    ) -> Result<Vec<RawChange>> {
        let start = self.trend_start();
        Ok(self
            .readable_changes(stored, names, campaign)?
            .into_iter()
            .filter(|change| change.day >= start)
            .collect())
    }

    pub fn readable_changes(
        &self,
        stored: &[AccountChange],
        names: &BTreeMap<i64, String>,
        campaign: Option<i64>,
    ) -> Result<Vec<RawChange>> {
        let mut changes = Vec::new();
        for change in stored
            .iter()
            .filter(|change| campaign.is_none() || change.campaign_id == campaign)
        {
            if let Some(readable) = self.readable(change, names)? {
                changes.push(RawChange {
                    day: readable.0,
                    campaign: if campaign.is_some() {
                        String::new()
                    } else {
                        readable.1
                    },
                    text: readable.2,
                });
            }
        }
        Ok(changes)
    }

    pub fn changes(&self, raw: &[RawChange], translations: &Translations) -> Result<Vec<Change>> {
        raw.iter()
            .map(|change| {
                let text = translations.shown(&change.text);
                let label_source = charts::change_label(&change.text);
                let label = (translations.language() != Language::En && !text.english)
                    .then(|| translations.shown(&label_source))
                    .filter(|label| !label.english);
                let write = |text: &str| -> Result<String> {
                    if change.campaign.is_empty() {
                        Ok(text.to_owned())
                    } else {
                        say!(
                            self.text,
                            change_of_campaign,
                            campaign = &change.campaign,
                            text = text
                        )
                    }
                };
                Ok(Change {
                    day: change.day,
                    text: write(text.text)?,
                    label: label.map(|label| write(label.text)).transpose()?,
                    english: text.english,
                })
            })
            .collect()
    }

    fn readable(
        &self,
        change: &AccountChange,
        names: &BTreeMap<i64, String>,
    ) -> Result<Option<(i64, String, String)>> {
        let Some(readable) = crate::google_ads::change_text(change, names, self.currency) else {
            return Ok(None);
        };
        if !self.judged && readable.text.to_lowercase().contains("conversion") {
            return Ok(None);
        }
        let day = day_number(
            chrono::NaiveDate::parse_from_str(&readable.date, "%Y-%m-%d")
                .map_err(|_| Error::Message("Invalid Account change date".into()))?,
        );
        Ok(Some((day, readable.campaign, readable.text)))
    }
}

pub struct RawChange {
    pub day: i64,
    pub campaign: String,
    pub text: String,
}

impl RawChange {
    pub fn wanted(changes: &[RawChange], wanted: &mut Wanted) {
        for change in changes {
            wanted
                .text(&change.text)
                .text(&charts::change_label(&change.text));
        }
    }
}

pub(super) fn window_view(
    text: &Text,
    path: &str,
    window: Window,
    end: i64,
    metric: TrendMetric,
) -> Result<Value> {
    let choices = [Window::SevenDays, Window::ThirtyDays].map(|choice| {
        let mut href = with_window(path, choice);
        if metric != TrendMetric::Spend {
            let join = if href.contains('?') { '&' } else { '?' };
            href = format!("{href}{join}metric={}", metric.key());
        }
        json!({"days": choice.days(), "href": href, "current": choice == window})
    });
    Ok(json!({
        "days": window.days(),
        "span": view::span(text, end - window.days() + 1, end)?,
        "choices": choices,
    }))
}

pub(super) fn metric_links(
    text: &Text,
    path: &str,
    window: Window,
    metrics: &[MetricView],
    chosen: TrendMetric,
    judged: bool,
) -> Vec<Value> {
    let base = with_window(path, window);
    let join = if base.contains('?') { '&' } else { '?' };
    metrics
        .iter()
        .map(|shown| {
            TrendMetric::offered(judged)
                .iter()
                .find(|metric| metric.label(text) == shown.label)
                .map_or(Value::Null, |metric| {
                    json!({
                        "href": format!("{base}{join}metric={}#trend", metric.key()),
                        "label": metric.per_day(text),
                        "current": *metric == chosen,
                    })
                })
        })
        .collect()
}
