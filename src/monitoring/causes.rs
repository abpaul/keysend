use std::collections::{BTreeMap, BTreeSet};

use chrono::NaiveDate;
use loco_rs::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{
    campaign_text::Campaigns,
    detection::{CampaignMovement, Detection, MetricName, Movement},
    metrics::{self, last_settled_conversion_day, sum, CampaignDay, WindowMetrics},
    tracking::MIN_CONVERSIONS,
    update,
};
use crate::{
    ai::{Claim, Stated},
    google_ads::{self, ChangeKind},
    language::{english, Text},
    workspace::{
        history::{date_of, day_number, AccountChange, Campaign},
        monitoring::{DeviceTotal, SearchShareTotal, SearchShares},
        WorkspaceStore,
    },
};

pub const CONCENTRATION: f64 = 0.6;
pub const NEAR_ZERO: f64 = 0.1;
pub const MIN_TRACKING_CLICKS: f64 = 100.0;
pub const MIN_SHARE_IMPRESSIONS: f64 = 100.0;
pub const MIN_SHARE_DAYS: i64 = 5;
pub const SHARE_RISE: f64 = 0.10;
pub const SHARE_FLOOR: f64 = 0.20;
pub const MIN_DAYS_AFTER: i64 = 2;
pub const BUDGET_BINDING: f64 = 0.8;

pub const PROMPT: &str = "Causes. The input lists the causes that the software found: one list for the summary and one list for each Campaign, each item with an id, a level, its evidence and its anchors. Set the cause field of a text to the id of the one listed item that the text states, or to null when the text states none. Never state a cause that is not listed, never state two causes in one text, and never state a cause of another Campaign. An item with the level confirmed is a fact: the text may connect it with because, after or due to. An item with the level likely is not a fact: the sentence that holds the causal word must also say most likely or probably, for example: Clicks fell most likely because the daily budget was lowered on 3 October. An item with the level contribution is a share of a movement, not a cause: give it with its numbers and use no causal word for it. When you state a cause, write one of its anchors exactly as given, in the same sentence as the causal word, for example the date of the account change. Take numbers and names from the input or from the evidence of the item that the text states. When the cause field is null, state no cause and connect an account change with the word after only.";

/// A Contribution is the share of a movement that one Campaign or device holds. It is a fact
/// about the movement, not a reason for it, so it unlocks no causal words.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Confirmed,
    Likely,
    Contribution,
}

impl Level {
    fn as_str(self) -> &'static str {
        match self {
            Self::Confirmed => "confirmed",
            Self::Likely => "likely",
            Self::Contribution => "contribution",
        }
    }
}

/// The variants are in the order in which an analyst checks them.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Tracking,
    CampaignStopped,
    CampaignStarted,
    AccountChange,
    BudgetLimit,
    RankLost,
    Concentration,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Candidate {
    pub id: String,
    pub kind: Kind,
    pub level: Level,
    pub metric: MetricName,
    pub campaign_id: Option<i64>,
    pub evidence: String,
    /// A text that states this cause names one of these, in the sentence that holds a causal word.
    #[serde(default)]
    pub anchors: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Cause {
    pub level: Level,
    pub evidence: String,
}

impl From<&Candidate> for Cause {
    fn from(candidate: &Candidate) -> Self {
        Self {
            level: candidate.level,
            evidence: candidate.evidence.clone(),
        }
    }
}

pub struct Inputs<'a> {
    pub days: &'a [CampaignDay],
    pub changes: &'a [AccountChange],
    pub names: BTreeMap<i64, String>,
    pub shares_now: Vec<SearchShareTotal>,
    pub shares_before: Vec<SearchShareTotal>,
    pub devices: BTreeMap<(i64, bool), (Vec<DeviceTotal>, Vec<DeviceTotal>)>,
}

pub async fn gather<'a>(
    store: &WorkspaceStore<'_>,
    id: &str,
    days: &'a [CampaignDay],
    changes: &'a [AccountChange],
    campaigns: &[Campaign],
    detection: &Detection,
) -> Result<Inputs<'a>> {
    let end = detection.day - 1;
    let mut inputs = Inputs {
        days,
        changes,
        names: campaigns
            .iter()
            .map(|campaign| (campaign.id, campaign.name.clone()))
            .collect(),
        shares_now: Vec::new(),
        shares_before: Vec::new(),
        devices: BTreeMap::new(),
    };
    if detection.campaigns.is_empty() {
        return Ok(inputs);
    }
    inputs.shares_now = store.search_share_totals(id, end - 6, end).await?;
    inputs.shares_before = store.search_share_totals(id, end - 13, end - 7).await?;
    for campaign in update::campaigns(detection) {
        for movement in &campaign.movements {
            let lagged = movement.metric == MetricName::Conversions;
            let last = if lagged {
                last_settled_conversion_day(end)
            } else {
                end
            };
            if inputs.devices.contains_key(&(campaign.campaign_id, lagged)) {
                continue;
            }
            let now = store
                .device_totals(id, last - 6, last, Some(campaign.campaign_id))
                .await?;
            let before = store
                .device_totals(id, last - 13, last - 7, Some(campaign.campaign_id))
                .await?;
            inputs
                .devices
                .insert((campaign.campaign_id, lagged), (now, before));
        }
    }
    Ok(inputs)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    Up,
    Down,
}

impl Direction {
    fn of(change: f64) -> Option<Self> {
        if change > 0.0 {
            Some(Self::Up)
        } else if change < 0.0 {
            Some(Self::Down)
        } else {
            None
        }
    }

    fn word(self) -> &'static str {
        match self {
            Self::Up => "rise",
            Self::Down => "fall",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Class {
    Budget,
    Status,
    Bid,
    Target,
}

impl Class {
    fn slug(self) -> &'static str {
        match self {
            Self::Budget => "budget",
            Self::Status => "status",
            Self::Bid => "bid-strategy",
            Self::Target => "target",
        }
    }
}

struct Classified {
    class: Class,
    direction: Option<Direction>,
    budget: Option<(f64, f64)>,
}

fn moved_by(from: Option<f64>, to: Option<f64>) -> Option<Direction> {
    Direction::of(to? - from?)
}

fn classify(kind: ChangeKind) -> Option<Classified> {
    match kind {
        ChangeKind::Budget { from, to } => Some(Classified {
            class: Class::Budget,
            direction: Some(moved_by(from, to)?),
            budget: from.zip(to),
        }),
        ChangeKind::Status { to, .. } => {
            let direction = match to.as_str() {
                "enabled" => Direction::Up,
                "paused" | "removed" => Direction::Down,
                _ => return None,
            };
            Some(Classified {
                class: Class::Status,
                direction: Some(direction),
                budget: None,
            })
        }
        ChangeKind::BidStrategy { .. } => Some(Classified {
            class: Class::Bid,
            direction: None,
            budget: None,
        }),
        ChangeKind::TargetCost { from, to } => Some(Classified {
            class: Class::Target,
            direction: Some(moved_by(from, to)?),
            budget: None,
        }),
    }
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

fn device_name(device: i64) -> &'static str {
    let written = &english().written;
    match device {
        1 => written.device_mobile,
        2 => written.device_desktop,
        3 => written.device_tablet,
        4 => written.device_tv,
        _ => written.device_other,
    }
}

fn percent(ratio: f64) -> String {
    format!("{}%", metrics::english_number((ratio * 100.0).round(), 0))
}

struct Subject<'a> {
    campaign: Option<i64>,
    metrics: &'a WindowMetrics,
    movements: &'a [Movement],
    judged: bool,
}

struct Context<'a> {
    detection: &'a Detection,
    inputs: &'a Inputs<'a>,
    end: i64,
}

impl Context<'_> {
    fn last(&self, metric: MetricName) -> i64 {
        if metric == MetricName::Conversions {
            last_settled_conversion_day(self.end)
        } else {
            self.end
        }
    }

    fn pair(&self, metric: MetricName, campaign: Option<i64>) -> (f64, f64) {
        let last = self.last(metric);
        let days = self.inputs.days;
        (
            metric.value(&sum(days, last - 6, last, campaign)),
            metric.value(&sum(days, last - 13, last - 7, campaign)),
        )
    }

    fn daily(&self, metric: MetricName, campaign: Option<i64>, from: i64, to: i64) -> Vec<f64> {
        (from..=to)
            .map(|day| metric.value(&sum(self.inputs.days, day, day, campaign)))
            .collect()
    }

    fn show(&self, metric: MetricName, value: f64) -> String {
        match metric {
            MetricName::Spend => metrics::english_money(value, &self.detection.currency),
            _ => metrics::english_count(value),
        }
    }

    fn candidate(
        subject: &Subject,
        slug: String,
        kind: Kind,
        level: Level,
        metric: MetricName,
        evidence: String,
        anchors: &[&str],
    ) -> Candidate {
        let scope = subject
            .campaign
            .map_or_else(|| "account".to_owned(), |id| id.to_string());
        Candidate {
            id: format!("{scope}/{slug}"),
            kind,
            level,
            metric,
            campaign_id: subject.campaign,
            evidence,
            anchors: anchors.iter().map(|anchor| (*anchor).to_owned()).collect(),
        }
    }

    fn tracking(&self, subject: &Subject) -> Option<Candidate> {
        let moved = subject
            .movements
            .iter()
            .find(|movement| movement.metric == MetricName::Conversions)
            .filter(|movement| movement.current < movement.previous)?;
        let clicks_moved = subject
            .movements
            .iter()
            .any(|movement| movement.metric == MetricName::Clicks);
        let clicks = subject.metrics.current.clicks;
        (moved.previous >= MIN_CONVERSIONS
            && moved.current <= moved.previous * NEAR_ZERO
            && !clicks_moved
            && clicks >= MIN_TRACKING_CLICKS)
            .then(|| {
                Self::candidate(
                    subject,
                    "tracking/conversions".into(),
                    Kind::Tracking,
                    Level::Likely,
                    MetricName::Conversions,
                    format!(
                        "Conversions fell from {} to {} in the 7 days that ended 2 days ago, while Clicks stayed inside their normal range at {} in the last 7 days.",
                        metrics::english_count(moved.previous),
                        metrics::english_count(moved.current),
                        metrics::english_count(clicks)
                    ),
                    &["tracking"],
                )
            })
    }

    fn served(&self, campaign: &CampaignMovement, subject: &Subject) -> Vec<Candidate> {
        let (current, previous) = (
            self.show(MetricName::Spend, campaign.metrics.current.spend),
            self.show(MetricName::Spend, campaign.metrics.previous.spend),
        );
        campaign
            .reasons
            .iter()
            .filter_map(|reason| {
                if let Some(date) = reason.strip_prefix("Campaign stopped serving on ") {
                    Some((
                        "stopped",
                        Kind::CampaignStopped,
                        format!("Campaign stopped serving on {date}, and Spend was {current} in the last 7 days, against {previous} in the previous 7 days."),
                    ))
                } else if reason == "Campaign started in the last 7 days" {
                    Some((
                        "started",
                        Kind::CampaignStarted,
                        format!("Campaign started serving in the last 7 days, and Spend was {current}, against {previous} in the previous 7 days."),
                    ))
                } else {
                    None
                }
            })
            .map(|(slug, kind, evidence)| {
                Self::candidate(
                    subject,
                    format!("{slug}/spend"),
                    kind,
                    Level::Confirmed,
                    MetricName::Spend,
                    evidence,
                    &[slug],
                )
            })
            .collect()
    }

    fn means(&self, metric: MetricName, campaign: Option<i64>, from: i64, to: i64) -> (f64, f64) {
        (
            mean(&self.daily(metric, campaign, from, to)),
            mean(&self.daily(metric, campaign, from - 7, to - 7)),
        )
    }

    /// Compares each day after the change with the same weekday one week earlier, so that a
    /// weekly pattern is not read as an effect of the change.
    fn settled_after(
        &self,
        metric: MetricName,
        campaign: Option<i64>,
        change_day: i64,
        moved: Direction,
    ) -> Option<(f64, f64)> {
        let last = self.last(metric);
        if change_day < last - 6 || change_day > last - MIN_DAYS_AFTER {
            return None;
        }
        let (after, previous) = self.means(metric, campaign, change_day + 1, last);
        let gap = after - previous;
        if Direction::of(gap) != Some(moved) {
            return None;
        }
        if change_day > last - 6 {
            let (before, earlier) = self.means(metric, campaign, last - 6, change_day - 1);
            if (before - earlier).abs() * 2.0 > gap.abs() {
                return None;
            }
        }
        Some((after, previous))
    }

    fn budget_limited_spend(
        &self,
        campaign: Option<i64>,
        change_day: i64,
        (from, to): (f64, f64),
        direction: Direction,
    ) -> bool {
        let spend =
            |first: i64, last: i64| mean(&self.daily(MetricName::Spend, campaign, first, last));
        campaign.is_some()
            && match direction {
                Direction::Up => spend(change_day - 7, change_day - 1) >= BUDGET_BINDING * from,
                Direction::Down => spend(change_day + 1, self.end) >= BUDGET_BINDING * to,
            }
    }

    fn account_changes(&self, subject: &Subject) -> Vec<Candidate> {
        let mut changes = self
            .inputs
            .changes
            .iter()
            .filter(|change| change.campaign_id == subject.campaign)
            .collect::<Vec<_>>();
        changes.sort_by(|a, b| b.changed_at.cmp(&a.changed_at));
        let mut seen = BTreeSet::new();
        let mut found = Vec::new();
        for change in changes {
            let Some(day) = change
                .changed_at
                .get(..10)
                .and_then(|date| NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
                .map(day_number)
            else {
                continue;
            };
            let Some(Classified {
                class,
                direction,
                budget,
            }) = google_ads::change_kind(change).and_then(classify)
            else {
                continue;
            };
            if class == Class::Target && !subject.judged {
                continue;
            }
            let Some(readable) =
                google_ads::change_text(change, &self.inputs.names, &self.detection.currency)
            else {
                continue;
            };
            for movement in subject.movements {
                let Some(moved) = Direction::of(movement.current - movement.previous) else {
                    continue;
                };
                if direction.is_some_and(|direction| direction != moved) {
                    continue;
                }
                let Some((after, previous)) =
                    self.settled_after(movement.metric, subject.campaign, day, moved)
                else {
                    continue;
                };
                if !seen.insert((class, movement.metric)) {
                    continue;
                }
                let on_spend_or_clicks =
                    matches!(movement.metric, MetricName::Spend | MetricName::Clicks);
                let confirmed = on_spend_or_clicks
                    && match class {
                        Class::Status => true,
                        Class::Budget => budget.is_some_and(|budget| {
                            self.budget_limited_spend(subject.campaign, day, budget, moved)
                        }),
                        Class::Bid | Class::Target => false,
                    };
                let date = date_of(day)
                    .map(|date| date.format("%-d %B").to_string())
                    .unwrap_or_default();
                found.push(Self::candidate(
                    subject,
                    format!(
                        "account-change-{}/{}/{}",
                        class.slug(),
                        movement.metric.as_str().to_lowercase(),
                        date_of(day).map(|date| date.to_string()).unwrap_or_default()
                    ),
                    Kind::AccountChange,
                    if confirmed {
                        Level::Confirmed
                    } else {
                        Level::Likely
                    },
                    movement.metric,
                    format!(
                        "{} on {date}. {} averaged {} a day after it, against {} a day on the same weekdays of the week before.",
                        readable.text,
                        movement.metric.as_str(),
                        self.show(movement.metric, after),
                        self.show(movement.metric, previous)
                    ),
                    &[&date],
                ));
            }
        }
        found
    }

    fn lost_share(
        &self,
        campaign: &CampaignMovement,
        subject: &Subject,
        kind: Kind,
    ) -> Option<Candidate> {
        let metric = [MetricName::Clicks, MetricName::Conversions]
            .into_iter()
            .find(|metric| {
                campaign.movements.iter().any(|movement| {
                    movement.metric == *metric && movement.current < movement.previous
                })
            })?;
        let own = |totals: &[SearchShareTotal]| {
            totals
                .iter()
                .find(|total| total.campaign_id == campaign.campaign_id)
                .filter(|total| total.limit_days == 0 && total.exact_days >= MIN_SHARE_DAYS)
                .and_then(|total| total.shares)
        };
        let (now, before) = (
            own(&self.inputs.shares_now)?,
            own(&self.inputs.shares_before)?,
        );
        let (name, pick): (&str, fn(&SearchShares) -> f64) = if kind == Kind::BudgetLimit {
            ("budget", |shares| shares.lost_budget)
        } else {
            ("rank", |shares| shares.lost_rank)
        };
        let (now, before) = (pick(&now), pick(&before));
        let impressions = |from: i64, to: i64| {
            sum(self.inputs.days, from, to, Some(campaign.campaign_id)).impressions
        };
        (now - before >= SHARE_RISE
            && now >= SHARE_FLOOR
            && impressions(self.end - 6, self.end) >= MIN_SHARE_IMPRESSIONS
            && impressions(self.end - 13, self.end - 7) >= MIN_SHARE_IMPRESSIONS)
            .then(|| {
                Self::candidate(
                    subject,
                    format!("{}/{}", if kind == Kind::BudgetLimit { "budget-limit" } else { "rank-lost" }, metric.as_str().to_lowercase()),
                    kind,
                    Level::Likely,
                    metric,
                    format!(
                        "Share of impressions lost to {name} was {} in the last 7 days, against {} in the previous 7 days.",
                        percent(now),
                        percent(before)
                    ),
                    &[name],
                )
            })
    }

    fn of_campaign(&self, campaign: &CampaignMovement) -> Vec<Candidate> {
        let subject = Subject {
            campaign: Some(campaign.campaign_id),
            metrics: &campaign.metrics,
            movements: &campaign.movements,
            judged: campaign.metrics.current.conversions.is_some(),
        };
        let mut found = Vec::new();
        found.extend(self.tracking(&subject));
        found.extend(self.served(campaign, &subject));
        found.extend(self.account_changes(&subject));
        for kind in [Kind::BudgetLimit, Kind::RankLost] {
            found.extend(self.lost_share(campaign, &subject, kind));
        }
        found
    }

    fn lifts(&self, own: &[(&CampaignMovement, Vec<Candidate>)]) -> Vec<(i64, Candidate)> {
        let mut found = Vec::new();
        for (campaign, candidates) in own {
            for candidate in candidates
                .iter()
                .filter(|candidate| candidate.kind != Kind::Tracking)
            {
                let Some(movement) = self
                    .detection
                    .movements
                    .iter()
                    .find(|movement| movement.metric == candidate.metric)
                else {
                    continue;
                };
                let total = movement.current - movement.previous;
                let Some(direction) = Direction::of(total) else {
                    continue;
                };
                let (current, previous) = self.pair(candidate.metric, Some(campaign.campaign_id));
                let ratio = (current - previous) / total;
                if !(CONCENTRATION..=1.0).contains(&ratio) {
                    continue;
                }
                found.push((
                    campaign.campaign_id,
                    Candidate {
                        id: format!("account/{}", candidate.id),
                        campaign_id: None,
                        evidence: format!(
                            "{}: {} It accounts for {} of the {} in {}.",
                            campaign.name,
                            candidate.evidence,
                            percent(ratio),
                            direction.word(),
                            candidate.metric.as_str()
                        ),
                        ..candidate.clone()
                    },
                ));
            }
        }
        found
    }

    fn campaigns_concentration(&self, lifted: &BTreeSet<(i64, MetricName)>) -> Vec<Candidate> {
        let account = Subject {
            campaign: None,
            metrics: &self.detection.metrics,
            movements: &self.detection.movements,
            judged: self.detection.judge_by_conversions,
        };
        let mut found = Vec::new();
        for movement in account.movements {
            let total = movement.current - movement.previous;
            let Some(direction) = Direction::of(total) else {
                continue;
            };
            let parts = self
                .inputs
                .names
                .iter()
                .map(|(id, name)| {
                    let (current, previous) = self.pair(movement.metric, Some(*id));
                    (
                        *id,
                        name,
                        current - previous,
                        current > 0.0 || previous > 0.0,
                    )
                })
                .collect::<Vec<_>>();
            if parts.iter().filter(|part| part.3).count() < 2 {
                continue;
            }
            let Some((id, name, change, _)) = parts
                .iter()
                .filter(|(id, _, change, _)| {
                    (CONCENTRATION..=1.0).contains(&(change / total))
                        && !lifted.contains(&(*id, movement.metric))
                })
                .max_by(|a, b| a.2.abs().total_cmp(&b.2.abs()))
            else {
                continue;
            };
            found.push(Self::candidate(
                &account,
                format!(
                    "concentration/{}/{id}",
                    movement.metric.as_str().to_lowercase()
                ),
                Kind::Concentration,
                Level::Contribution,
                movement.metric,
                format!(
                    "{name} accounts for {} of the {} in {}: {} of {}.",
                    percent(change / total),
                    direction.word(),
                    movement.metric.as_str(),
                    self.show(movement.metric, change.abs()),
                    self.show(movement.metric, total.abs())
                ),
                &[],
            ));
        }
        found
    }

    fn devices(&self, campaign: &CampaignMovement) -> Vec<Candidate> {
        let subject = Subject {
            campaign: Some(campaign.campaign_id),
            metrics: &campaign.metrics,
            movements: &campaign.movements,
            judged: campaign.metrics.current.conversions.is_some(),
        };
        let mut found = Vec::new();
        for movement in &campaign.movements {
            let Some((now, before)) = self.inputs.devices.get(&(
                campaign.campaign_id,
                movement.metric == MetricName::Conversions,
            )) else {
                continue;
            };
            let read = |total: &DeviceTotal| match movement.metric {
                MetricName::Spend => total.spend,
                MetricName::Clicks => total.clicks,
                MetricName::Conversions => total.conversions,
            };
            let mut devices: BTreeMap<i64, (f64, f64)> = BTreeMap::new();
            for total in now {
                devices.entry(total.device).or_default().0 += read(total);
            }
            for total in before {
                devices.entry(total.device).or_default().1 += read(total);
            }
            let total = devices
                .values()
                .map(|(current, previous)| current - previous)
                .sum::<f64>();
            let Some(direction) = Direction::of(total) else {
                continue;
            };
            if Direction::of(movement.current - movement.previous) != Some(direction)
                || devices
                    .values()
                    .filter(|(current, previous)| *current > 0.0 || *previous > 0.0)
                    .count()
                    < 2
            {
                continue;
            }
            let Some((device, change)) = devices
                .iter()
                .map(|(device, (current, previous))| (*device, current - previous))
                .filter(|(_, change)| (CONCENTRATION..=1.0).contains(&(change / total)))
                .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
            else {
                continue;
            };
            found.push(Self::candidate(
                &subject,
                format!(
                    "concentration/{}/device-{device}",
                    movement.metric.as_str().to_lowercase()
                ),
                Kind::Concentration,
                Level::Contribution,
                movement.metric,
                format!(
                    "{} accounts for {} of the {} in {}: {} of {}.",
                    device_name(device),
                    percent(change / total),
                    direction.word(),
                    movement.metric.as_str(),
                    self.show(movement.metric, change.abs()),
                    self.show(movement.metric, total.abs())
                ),
                &[],
            ));
        }
        found
    }
}

pub fn candidates(detection: &Detection, inputs: &Inputs) -> Vec<Candidate> {
    let context = Context {
        detection,
        inputs,
        end: detection.day - 1,
    };
    let account = Subject {
        campaign: None,
        metrics: &detection.metrics,
        movements: &detection.movements,
        judged: detection.judge_by_conversions,
    };
    let mut found = Vec::new();
    found.extend(context.tracking(&account));
    found.extend(context.account_changes(&account));
    let own = detection
        .campaigns
        .iter()
        .map(|campaign| (campaign, context.of_campaign(campaign)))
        .collect::<Vec<_>>();
    let lifts = context.lifts(&own);
    let lifted = lifts
        .iter()
        .map(|(campaign, candidate)| (*campaign, candidate.metric))
        .collect::<BTreeSet<_>>();
    found.extend(lifts.into_iter().map(|(_, candidate)| candidate));
    found.extend(context.campaigns_concentration(&lifted));
    for (campaign, candidates) in own {
        found.extend(candidates);
        found.extend(context.devices(campaign));
    }
    found.sort_by_key(|candidate| (candidate.kind, candidate.metric));
    found
}

pub fn first(candidates: &[Candidate], campaign: Option<i64>) -> Option<&Candidate> {
    candidates
        .iter()
        .find(|candidate| candidate.campaign_id == campaign)
}

pub fn chosen<'a>(
    candidates: &'a [Candidate],
    id: Option<&str>,
    campaign: Option<i64>,
) -> std::result::Result<Option<&'a Candidate>, &'static str> {
    let Some(id) = id else {
        return Ok(None);
    };
    candidates
        .iter()
        .find(|candidate| candidate.id == id && candidate.campaign_id == campaign)
        .map(Some)
        .ok_or("unlisted cause")
}

pub fn label(text: &Text, level: Level) -> &'static str {
    match level {
        Level::Confirmed => text.written.cause_confirmed,
        Level::Likely => text.written.cause_likely,
        Level::Contribution => text.written.cause_contribution,
    }
}

pub fn view(text: &Text, cause: &Cause, campaigns: &Campaigns, about: Option<i64>) -> Value {
    json!({"label":label(text, cause.level),"evidence":campaigns.parts(&cause.evidence, about)})
}

pub(crate) fn stated<'a>(candidate: Option<&'a Candidate>, names: &'a [&'a str]) -> Stated<'a> {
    let claim = match candidate.map(|candidate| candidate.level) {
        None | Some(Level::Contribution) => Claim::Nothing,
        Some(Level::Confirmed) => Claim::Confirmed,
        Some(Level::Likely) => Claim::Likely,
    };
    Stated {
        claim,
        anchors: candidate.map_or(&[], |candidate| candidate.anchors.as_slice()),
        names,
    }
}

/// The sentence that the fallback writes for a cause: code-written words with the level of the
/// cause. A Contribution states no reason, so it has none.
pub fn statement(candidate: &Candidate, rose: Option<bool>) -> Option<String> {
    let moved = match rose {
        Some(true) => "rose",
        Some(false) => "fell",
        None => "moved",
    };
    let hedge = if candidate.level == Level::Likely {
        ", most likely"
    } else {
        ""
    };
    let reason = match candidate.kind {
        Kind::Concentration => return None,
        Kind::Tracking => "conversion tracking stopped recording them".to_owned(),
        Kind::CampaignStopped => "the Campaign stopped serving".to_owned(),
        Kind::CampaignStarted => "the Campaign started serving".to_owned(),
        Kind::AccountChange => format!("of the Account change on {}", candidate.anchors.first()?),
        Kind::BudgetLimit => "the Campaign lost more impressions to its budget".to_owned(),
        Kind::RankLost => "the Campaign lost more impressions to its rank".to_owned(),
    };
    Some(format!(
        "{} {moved}{hedge} because {reason}.",
        candidate.metric.as_str()
    ))
}

pub fn with_evidence(facts: &Value, candidate: Option<&Candidate>) -> Value {
    match candidate {
        Some(candidate) => json!([facts, candidate.evidence]),
        None => facts.clone(),
    }
}

pub fn listed(candidates: &[Candidate], campaign: Option<i64>) -> Value {
    candidates
        .iter()
        .filter(|candidate| candidate.campaign_id == campaign)
        .map(|candidate| {
            json!({"id":candidate.id,"level":candidate.level.as_str(),"evidence":candidate.evidence,"anchors":candidate.anchors})
        })
        .collect()
}

fn choice(ids: Vec<&str>) -> Value {
    if ids.is_empty() {
        json!({"type":"null"})
    } else {
        let mut allowed = ids.into_iter().map(Value::from).collect::<Vec<_>>();
        allowed.push(Value::Null);
        json!({"type":["string","null"],"enum":allowed})
    }
}

pub fn extend_request(payload: &mut Value, candidates: &[Candidate], campaigns: &[i64]) {
    let ids = |wanted: &dyn Fn(Option<i64>) -> bool| {
        candidates
            .iter()
            .filter(|candidate| wanted(candidate.campaign_id))
            .map(|candidate| candidate.id.as_str())
            .collect::<Vec<_>>()
    };
    let account = choice(ids(&|campaign| campaign.is_none()));
    let lines = choice(ids(&|campaign| {
        campaign.is_some_and(|id| campaigns.contains(&id))
    }));
    let schema = &mut payload["text"]["format"]["schema"];
    if let Some(required) = schema["required"].as_array_mut() {
        required.push("cause".into());
    }
    if let Some(required) = schema["properties"]["lines"]["items"]["required"].as_array_mut() {
        required.push("cause".into());
    }
    schema["properties"]["cause"] = account;
    schema["properties"]["lines"]["items"]["properties"]["cause"] = lines;
    if let Some(instructions) = payload["instructions"].as_str().map(str::to_owned) {
        payload["instructions"] = format!("{instructions} {PROMPT}").into();
    }
}

#[cfg(test)]
mod tests;
