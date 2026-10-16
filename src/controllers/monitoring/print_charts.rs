use std::collections::BTreeMap;

use loco_rs::prelude::*;

use super::{
    breakdown_pages,
    chart_pages::{RawChange, Source},
};
use crate::{
    language::Text,
    monitoring::{
        campaign_settings::Resolved,
        charts::{self, Chart, Frame, TrendMetric, HISTORY_DAYS},
        export::{CampaignDeepCheck, ChartChanges, ChartImage, DatedChange, DeepCharts},
        follow_up,
        metrics::{CampaignDay, Window},
        view,
    },
    translation::Translations,
    workspace::{history::Campaign, WorkspaceStore},
};

fn image(chart: Chart, frame: Frame) -> ChartImage {
    ChartImage {
        svg: charts::standalone(&chart, frame),
        sentence: chart.sentence.words(),
        title: chart.title,
        note: None,
        changes: ChartChanges::NotShown,
    }
}

pub struct Collected<'a> {
    pub text: &'a Text,
    pub id: &'a str,
    pub days: &'a [CampaignDay],
    pub campaigns: &'a [Campaign],
    pub first_day: i64,
    pub end: i64,
    pub currency: &'a str,
    pub judged: bool,
    pub window: Window,
    pub translations: &'a Translations,
}

impl Collected<'_> {
    fn source(&self) -> Source<'_> {
        Source {
            text: self.text,
            id: self.id,
            days: self.days,
            from: first_read_day(self.end),
            end: self.end,
            first_day: self.first_day,
            judged: self.judged,
            currency: self.currency,
            window: self.window,
        }
    }
}

pub fn first_read_day(end: i64) -> i64 {
    end - HISTORY_DAYS + 1
}

pub async fn export_changes(
    store: &WorkspaceStore<'_>,
    collected: &Collected<'_>,
    campaign: Option<&Campaign>,
) -> Result<Vec<RawChange>> {
    let names = match campaign {
        Some(campaign) => BTreeMap::from([(campaign.id, campaign.name.clone())]),
        None => collected
            .campaigns
            .iter()
            .map(|campaign| (campaign.id, campaign.name.clone()))
            .collect(),
    };
    collected
        .source()
        .raw_changes(store, &names, campaign.map(|campaign| campaign.id))
        .await
}

pub async fn change_list(
    store: &WorkspaceStore<'_>,
    collected: &Collected<'_>,
    campaign: Option<&Campaign>,
    raw: &[RawChange],
    rules: &Resolved,
) -> Result<ChartChanges> {
    let source = collected.source();
    let names = match campaign {
        Some(campaign) => BTreeMap::from([(campaign.id, campaign.name.clone())]),
        None => collected
            .campaigns
            .iter()
            .map(|campaign| (campaign.id, campaign.name.clone()))
            .collect(),
    };
    let stored = source
        .stored_changes(store, follow_up::OTHER_CHANGE_DAYS)
        .await?;
    let follow_ups = follow_up::follow_ups(
        &stored,
        collected.days,
        &names,
        rules,
        collected.currency,
        collected.end + 1,
    );
    let changes = source.changes(raw, collected.translations)?;
    if campaign.is_none() && changes.is_empty() {
        return Ok(ChartChanges::NotShown);
    }
    Ok(ChartChanges::of(
        changes
            .iter()
            .zip(raw)
            .map(|(change, raw)| {
                Ok(DatedChange {
                    date: view::day_month(collected.text, change.day)?,
                    text: change.text.clone(),
                    follow_up: follow_ups
                        .iter()
                        .find(|found| found.change_day == raw.day && found.change == raw.text)
                        .map(|found| {
                            follow_up::page_text(collected.text, found, collected.currency)
                        })
                        .transpose()?,
                })
            })
            .collect::<Result<_>>()?,
    ))
}

fn trends(
    source: &Source<'_>,
    campaign: Option<i64>,
    changes: &[charts::Change],
    listed: ChartChanges,
) -> Result<Vec<ChartImage>> {
    let mut listed = Some(listed);
    TrendMetric::offered(source.judged)
        .iter()
        .map(|metric| {
            let mut trend = image(
                source.trend_for_print(campaign, *metric, changes)?,
                Frame::Trend,
            );
            if *metric == TrendMetric::Spend {
                trend.changes = listed.take().unwrap_or(ChartChanges::NotShown);
                if trend.svg.is_some() {
                    trend.note = Some(format!(
                        "{} {}",
                        source.text.written.explain_trend_band,
                        source.text.written.explain_trend_points,
                    ));
                }
            }
            Ok(trend)
        })
        .collect()
}

pub fn metric_sparklines(
    collected: &Collected<'_>,
    campaign: Option<i64>,
) -> Result<Vec<(&'static str, String)>> {
    Ok(TrendMetric::offered(collected.judged)
        .iter()
        .map(|metric| metric.label(collected.text))
        .zip(collected.source().metric_sparklines(campaign)?)
        .filter_map(|(label, svg)| Some((label, charts::standalone_spark(&svg?))))
        .collect())
}

pub fn campaign_sparklines(collected: &Collected<'_>) -> Result<BTreeMap<i64, String>> {
    Ok(collected
        .source()
        .sparklines()?
        .into_iter()
        .map(|(campaign, svg)| (campaign, charts::standalone_spark(&svg)))
        .collect())
}

pub async fn export_charts(
    store: &WorkspaceStore<'_>,
    collected: &Collected<'_>,
    changes: &[RawChange],
    rules: &Resolved,
) -> Result<(Vec<ChartImage>, DeepCharts)> {
    let source = collected.source();
    let listed = change_list(store, collected, None, changes, rules).await?;
    let changes = source.changes(changes, collected.translations)?;
    let mut images = trends(&source, None, &changes, listed)?;
    if collected.judged {
        let target = store.profile(collected.id).await?.target_cost;
        let mut weeks = image(
            source.cost_weeks(target.filter(|cost| *cost > 0.0))?,
            Frame::Weeks,
        );
        weeks.note = breakdown_pages::delay_note(collected.text, store, collected.id).await?;
        images.push(weeks);
    }
    images.push(image(
        source.campaign_shares(collected.campaigns, |_| String::new())?,
        Frame::Shares,
    ));
    if let Some(chart) = source
        .search_share(store, collected.campaigns, None, |_| String::new())
        .await?
    {
        images.push(image(chart, Frame::Stack));
    }
    images.push(image(
        source.device_shares(store, None).await?,
        Frame::Shares,
    ));
    let breakdowns = source.breakdowns(store).await?;
    if let Some(weeks) = breakdowns.action_weeks {
        images.push(image(weeks, Frame::Weeks));
    }
    let deep = DeepCharts {
        date: breakdowns.deep_check,
        charts: vec![
            image(breakdowns.hours, Frame::Grid),
            image(breakdowns.locations, Frame::Shares),
            image(breakdowns.ages, Frame::Shares),
            image(breakdowns.genders, Frame::Shares),
        ],
    };
    Ok((images, deep))
}

pub async fn campaign_export_charts(
    store: &WorkspaceStore<'_>,
    collected: &Collected<'_>,
    campaign: &Campaign,
    changes: &[RawChange],
    rules: &Resolved,
) -> Result<Vec<ChartImage>> {
    let source = collected.source();
    let listed = change_list(store, collected, Some(campaign), changes, rules).await?;
    let changes = source.changes(changes, collected.translations)?;
    let mut images = trends(&source, Some(campaign.id), &changes, listed)?;
    if collected.judged {
        images.push(image(
            source.campaign_cost_weeks(campaign.id, rules.effective(campaign.id).target)?,
            Frame::Weeks,
        ));
    }
    if let Some(chart) = source
        .search_share(
            store,
            std::slice::from_ref(campaign),
            Some(campaign.id),
            |_| String::new(),
        )
        .await?
    {
        images.push(image(chart, Frame::Stack));
    }
    images.push(image(
        source.device_shares(store, Some(campaign.id)).await?,
        Frame::Shares,
    ));
    images.push(image(
        source.network_shares(store, campaign.id).await?,
        Frame::Shares,
    ));
    Ok(images)
}

pub async fn campaign_deep_check(
    store: &WorkspaceStore<'_>,
    collected: &Collected<'_>,
    campaign: &Campaign,
    judged: bool,
) -> Result<Option<CampaignDeepCheck>> {
    Ok(breakdown_pages::deep_check(
        collected.text,
        store,
        collected.id,
        campaign.id,
        collected.currency,
        judged,
    )
    .await?
    .map(|(date, sections)| CampaignDeepCheck { date, sections }))
}
