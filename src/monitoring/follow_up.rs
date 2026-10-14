//! A follow-up checks how an Account change turned out: the 14 complete days after the change
//! against the 14 complete days before it. It is computed from the Account history, never stored
//! as a row of its own.

use std::collections::BTreeMap;

use loco_rs::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{
    campaign_settings::Rules,
    detection::{
        lagged_window, mean_deviation, CampaignMovement, Detection, MetricName, MIN_BASELINE_WEEKS,
        NEW_ACCOUNT_CHANGE, WEEK_DEVIATIONS,
    },
    metrics::{
        self, by_campaign, first_active_day, sum, CampaignDay, Metrics, CONVERSION_MIN_AGE_DAYS,
    },
};
use crate::{
    google_ads::{change_kind, change_text, ChangeKind},
    language::{english, say, say_amount, Text},
    workspace::history::{day_number, AccountChange, Campaign},
};

pub const FOLLOW_UP_DAYS: i64 = 14;
pub const DUE_AFTER_DAYS: i64 = FOLLOW_UP_DAYS + CONVERSION_MIN_AGE_DAYS;
pub const OTHER_CHANGE_DAYS: i64 = FOLLOW_UP_DAYS;
pub const VARIATION_PERIODS: usize = MIN_BASELINE_WEEKS;
pub const MIN_SERVING_DAYS: usize = 7;
pub const MIN_PERIOD_CONVERSIONS: f64 = 5.0;
pub const MIN_PERIOD_CLICKS: f64 = 100.0;
pub const COST_TOLERANCE: f64 = 0.25;
pub const SPEND_CUT_MARGIN: f64 = 0.10;
pub const COUNT_NOISE_DEVIATIONS: f64 = 2.0;
pub const MIN_COST_CHANGE_SHARE: f64 = 0.10;
pub const SEASONAL_SIMILARITY: f64 = 0.5;
pub const SEASONAL_MIN_MOVE: f64 = 0.10;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum Kind {
    BudgetRaised,
    BudgetLowered,
    BidStrategy,
    TargetCost,
    Paused,
    Enabled,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum Verdict {
    Worked,
    NotYet,
    NoClearEffect,
    CannotBeJudged,
    Reported,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum Reason {
    Confounded { others: usize },
    NotServing,
    FewResults,
    WithinVariation,
    ExtraSpendBroughtResults,
    SpendFellMoreThanResults,
    MovedBetter,
    MovedWorse,
    AccountMovedTheSameWay,
    Paused,
    Enabled,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Checked {
    pub campaign_id: i64,
    pub change_day: i64,
    pub change: String,
    pub kind: Kind,
    pub by_conversions: bool,
    pub before: Metrics,
    pub after: Metrics,
    pub verdict: Verdict,
    pub reason: Reason,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FollowUp {
    pub campaign_id: i64,
    pub change_day: i64,
    pub change: String,
    pub due_day: i64,
    pub checked: Option<Checked>,
}

pub fn due_day(change_day: i64) -> i64 {
    change_day + DUE_AFTER_DAYS
}

pub fn earliest_change_day(day: i64) -> i64 {
    day - DUE_AFTER_DAYS - OTHER_CHANGE_DAYS
}

fn kind_of(change: &AccountChange) -> Option<Kind> {
    match change_kind(change)? {
        ChangeKind::Budget {
            from: Some(from),
            to: Some(to),
        } if to > from => Some(Kind::BudgetRaised),
        ChangeKind::Budget {
            from: Some(from),
            to: Some(to),
        } if to < from => Some(Kind::BudgetLowered),
        ChangeKind::Status { to, .. } if to == "paused" => Some(Kind::Paused),
        ChangeKind::Status { to, .. } if to == "enabled" => Some(Kind::Enabled),
        ChangeKind::BidStrategy { .. } => Some(Kind::BidStrategy),
        ChangeKind::TargetCost {
            from: Some(from),
            to: Some(to),
        } if to != from => Some(Kind::TargetCost),
        _ => None,
    }
}

struct Readable<'a> {
    change: &'a AccountChange,
    campaign_id: i64,
    day: i64,
    text: String,
}

fn same_event(a: &AccountChange, b: &AccountChange) -> bool {
    a.changed_at == b.changed_at && a.resource_name == b.resource_name
}

fn day_of(change: &AccountChange) -> Option<i64> {
    chrono::NaiveDate::parse_from_str(change.changed_at.get(..10)?, "%Y-%m-%d")
        .ok()
        .map(day_number)
}

fn readable<'a>(
    changes: &'a [AccountChange],
    names: &BTreeMap<i64, String>,
    currency: &str,
) -> Vec<Readable<'a>> {
    changes
        .iter()
        .filter_map(|change| {
            let campaign_id = change.campaign_id?;
            let text = change_text(change, names, currency)?;
            let date = chrono::NaiveDate::parse_from_str(&text.date, "%Y-%m-%d").ok()?;
            Some(Readable {
                change,
                campaign_id,
                day: day_number(date),
                text: text.text,
            })
        })
        .collect()
}

/// Every stored change of the Campaign, whatever it changed, and every change that belongs to
/// no Campaign, inside the 28 days of the comparison.
fn other_changes(changes: &[AccountChange], read: &Readable) -> usize {
    changes
        .iter()
        .filter(|other| {
            other
                .campaign_id
                .is_none_or(|campaign| campaign == read.campaign_id)
                && !same_event(other, read.change)
                && day_of(other).is_some_and(|day| (day - read.day).abs() <= OTHER_CHANGE_DAYS)
        })
        .count()
}

pub fn follow_ups(
    changes: &[AccountChange],
    days: &[CampaignDay],
    names: &BTreeMap<i64, String>,
    rules: impl Rules,
    currency: &str,
    day: i64,
) -> Vec<FollowUp> {
    let readable = readable(changes, names, currency);
    let mut found = Vec::new();
    for read in &readable {
        let Some(kind) = kind_of(read.change) else {
            continue;
        };
        let judged = rules.campaign_judged(read.campaign_id);
        if kind == Kind::TargetCost && !judged {
            continue;
        }
        let due = due_day(read.day);
        let checked =
            (day >= due).then(|| check(read, kind, judged, other_changes(changes, read), days));
        found.push(FollowUp {
            campaign_id: read.campaign_id,
            change_day: read.day,
            change: read.text.clone(),
            due_day: due,
            checked,
        });
    }
    found.sort_by_key(|follow_up| (follow_up.change_day, follow_up.campaign_id));
    found
}

fn period(days: &[CampaignDay], start: i64, judged: bool) -> Metrics {
    let metrics = sum(days, start, start + FOLLOW_UP_DAYS - 1, None);
    if judged {
        metrics
    } else {
        metrics.without_conversions()
    }
}

fn serving_days(days: &[CampaignDay], start: i64) -> usize {
    (start..start + FOLLOW_UP_DAYS)
        .filter(|day| sum(days, *day, *day, None).serving())
        .count()
}

fn results(metrics: &Metrics, judged: bool) -> f64 {
    if judged {
        MetricName::Conversions.value(metrics)
    } else {
        metrics.clicks
    }
}

fn cost(metrics: &Metrics, judged: bool) -> Option<f64> {
    let results = results(metrics, judged);
    (results > 0.0).then(|| metrics.spend / results)
}

fn variation(series: &[f64], previous: f64, floor: f64) -> f64 {
    if series.len() >= VARIATION_PERIODS {
        let changes = series
            .windows(2)
            .map(|pair| (pair[0] - pair[1]).abs())
            .collect::<Vec<_>>();
        let (mean, deviation) = mean_deviation(&changes);
        (mean + WEEK_DEVIATIONS * deviation).max(floor)
    } else {
        (previous * NEW_ACCOUNT_CHANGE).max(floor)
    }
}

struct Normal {
    spend: f64,
    results: f64,
    cost: f64,
}

fn normal(read: &Readable, own: &[CampaignDay], judged: bool, before: &Metrics) -> Normal {
    let first = first_active_day(own).unwrap_or(read.day);
    let periods = (1..=VARIATION_PERIODS as i64)
        .map(|index| read.day - FOLLOW_UP_DAYS * index)
        .take_while(|start| *start >= first)
        .map(|start| period(own, start, judged))
        .collect::<Vec<_>>();
    let results_floor = if judged {
        MetricName::Conversions.floor()
    } else {
        MetricName::Clicks.floor()
    };
    let cost_before = cost(before, judged).unwrap_or_default();
    let spends = periods.iter().map(|m| m.spend).collect::<Vec<_>>();
    let counts = periods
        .iter()
        .map(|m| results(m, judged))
        .collect::<Vec<_>>();
    let costs = periods
        .iter()
        .filter_map(|m| cost(m, judged))
        .collect::<Vec<_>>();
    Normal {
        spend: variation(&spends, before.spend, MetricName::Spend.floor()),
        results: variation(&counts, results(before, judged), results_floor),
        cost: variation(&costs, cost_before, cost_before * MIN_COST_CHANGE_SHARE),
    }
}

fn beyond_noise(before: f64, after: f64) -> bool {
    (after - before).abs() > COUNT_NOISE_DEVIATIONS * (before + after).sqrt()
}

fn results_between(days: &[CampaignDay], start: i64, judged: bool, campaign: Option<i64>) -> f64 {
    let metrics = sum(days, start, start + FOLLOW_UP_DAYS - 1, campaign);
    results(&metrics, judged)
}

/// Whether the other Campaigns of the Ad Account, taken together, moved in the direction of
/// this Campaign by a similar ratio over the same two periods.
fn rest_of_account_moved_the_same_way(
    days: &[CampaignDay],
    read: &Readable,
    judged: bool,
    own: (f64, f64),
) -> bool {
    let rest = days
        .iter()
        .filter(|row| row.campaign_id != read.campaign_id)
        .cloned()
        .collect::<Vec<_>>();
    let before = results_between(&rest, read.day - FOLLOW_UP_DAYS, judged, None);
    let after = results_between(&rest, read.day + 1, judged, None);
    if before <= 0.0 || own.0 <= 0.0 || !beyond_noise(before, after) {
        return false;
    }
    let (own_move, rest_move) = (own.1 / own.0 - 1.0, after / before - 1.0);
    own_move * rest_move > 0.0
        && rest_move.abs() >= SEASONAL_MIN_MOVE
        && own_move.abs().min(rest_move.abs()) / own_move.abs().max(rest_move.abs())
            >= SEASONAL_SIMILARITY
}

fn check(
    read: &Readable,
    kind: Kind,
    judged: bool,
    others: usize,
    days: &[CampaignDay],
) -> Checked {
    let own = days
        .iter()
        .filter(|row| row.campaign_id == read.campaign_id)
        .cloned()
        .collect::<Vec<_>>();
    let own = own.as_slice();
    let before_start = read.day - FOLLOW_UP_DAYS;
    let after_start = read.day + 1;
    let before = period(own, before_start, judged);
    let after = period(own, after_start, judged);
    let fewest = results(&before, judged).min(results(&after, judged));
    let (verdict, reason) = match kind {
        _ if others > 0 => (Verdict::CannotBeJudged, Reason::Confounded { others }),
        Kind::Paused => (Verdict::Reported, Reason::Paused),
        Kind::Enabled => (Verdict::Reported, Reason::Enabled),
        _ if serving_days(own, before_start) < MIN_SERVING_DAYS
            || serving_days(own, after_start) < MIN_SERVING_DAYS
            || before.spend <= 0.0
            || after.spend <= 0.0 =>
        {
            (Verdict::CannotBeJudged, Reason::NotServing)
        }
        _ if fewest
            < if judged {
                MIN_PERIOD_CONVERSIONS
            } else {
                MIN_PERIOD_CLICKS
            } =>
        {
            (Verdict::CannotBeJudged, Reason::FewResults)
        }
        _ => {
            let decided = decide(
                kind,
                &before,
                &after,
                &normal(read, own, judged, &before),
                judged,
            );
            if matches!(decided.0, Verdict::Worked | Verdict::NotYet)
                && rest_of_account_moved_the_same_way(
                    days,
                    read,
                    judged,
                    (results(&before, judged), results(&after, judged)),
                )
            {
                (Verdict::NoClearEffect, Reason::AccountMovedTheSameWay)
            } else {
                decided
            }
        }
    };
    Checked {
        campaign_id: read.campaign_id,
        change_day: read.day,
        change: read.text.clone(),
        kind,
        by_conversions: judged,
        before,
        after,
        verdict,
        reason,
    }
}

fn decide(
    kind: Kind,
    before: &Metrics,
    after: &Metrics,
    normal: &Normal,
    judged: bool,
) -> (Verdict, Reason) {
    let (results_before, results_after) = (results(before, judged), results(after, judged));
    let cost_before = cost(before, judged).unwrap_or_default();
    let cost_after = cost(after, judged).unwrap_or_default();
    let beyond = beyond_noise(results_before, results_after);
    let results_rose = results_after - results_before > normal.results && beyond;
    let results_fell = results_before - results_after > normal.results && beyond;
    match kind {
        Kind::BudgetRaised => {
            if after.spend - before.spend <= normal.spend {
                (Verdict::NoClearEffect, Reason::WithinVariation)
            } else if cost_after > cost_before * (1.0 + COST_TOLERANCE) {
                (Verdict::NotYet, Reason::MovedWorse)
            } else if results_rose {
                (Verdict::Worked, Reason::ExtraSpendBroughtResults)
            } else {
                (Verdict::NoClearEffect, Reason::WithinVariation)
            }
        }
        Kind::BudgetLowered => {
            let spend_fall = (before.spend - after.spend) / before.spend;
            let results_fall = (results_before - results_after) / results_before;
            if before.spend - after.spend <= normal.spend {
                (Verdict::NoClearEffect, Reason::WithinVariation)
            } else if spend_fall - results_fall >= SPEND_CUT_MARGIN {
                (Verdict::Worked, Reason::SpendFellMoreThanResults)
            } else if results_fell {
                (Verdict::NotYet, Reason::MovedWorse)
            } else {
                (Verdict::NoClearEffect, Reason::WithinVariation)
            }
        }
        _ => {
            let cost_better = cost_before - cost_after > normal.cost;
            let cost_worse = cost_after - cost_before > normal.cost;
            let good = (cost_better && !results_fell) || (results_rose && !cost_worse);
            let bad = (cost_worse && !results_rose) || (results_fell && !cost_better);
            match (good, bad) {
                (true, false) => (Verdict::Worked, Reason::MovedBetter),
                (false, true) => (Verdict::NotYet, Reason::MovedWorse),
                _ => (Verdict::NoClearEffect, Reason::WithinVariation),
            }
        }
    }
}

pub const FOLLOW_UP_DUE: &str = "An Account change is due its follow-up";
pub const SUMMARY: &str = "KeySend checked how earlier Account changes turned out.";
pub const INSTRUCTIONS: &str = "A Campaign can have follow_ups: the software compared the 14 complete days after an account change with the 14 complete days before it. Write each follow-up inside the line of its Campaign, after the other facts of that line. State the verdict exactly as given in verdict: when it says It worked, say it worked; when it says It has not worked yet, say it has not worked yet; when it says There is no clear effect or KeySend cannot judge it, say that. Never change, soften or strengthen a verdict, and never say that the account change caused a result: connect them with after or since. When the verdict is none, state only the figures given. Use only the figures given in the follow-up.";

const NEGATIVE_CLAIMS: [&str; 5] = [
    "not worked",
    "not work",
    "not working",
    "not paid off",
    "not pay off",
];
const POSITIVE_CLAIMS: [&str; 5] = ["worked", "paid off", "pay off", "working", "successful"];
const NEUTRAL_CLAIMS: [&str; 5] = [
    "no clear effect",
    "cannot judge",
    "cannot be judged",
    "can not judge",
    "too early",
];
const UNGIVEN_CLAIMS: [&str; 5] = [
    "failed",
    "fail",
    "failure",
    "unsuccessful",
    "not successful",
];

impl Checked {
    fn results_name(&self, text: &Text) -> &'static str {
        if self.by_conversions {
            text.written.results_conversions
        } else {
            text.written.results_clicks
        }
    }

    fn cost_name(&self, text: &Text) -> &'static str {
        if self.by_conversions {
            text.written.metric_cost_per_conversion_lower
        } else {
            text.written.metric_cost_per_click_lower
        }
    }

    fn verdict_sentence(&self, text: &Text) -> Option<&'static str> {
        match self.verdict {
            Verdict::Worked => Some(text.written.follow_up_worked),
            Verdict::NotYet => Some(text.written.follow_up_not_yet),
            Verdict::NoClearEffect => Some(text.written.follow_up_no_clear_effect),
            Verdict::CannotBeJudged => Some(text.written.follow_up_cannot_judge),
            Verdict::Reported => None,
        }
    }

    fn figures(&self, text: &Text, currency: &str) -> Result<String> {
        let results = |metrics: &Metrics| text.count(results(metrics, self.by_conversions));
        let cost = |metrics: &Metrics| {
            cost(metrics, self.by_conversions).map_or_else(
                || text.written.not_available.to_owned(),
                |value| text.money(value, currency),
            )
        };
        say!(
            text,
            follow_up_figures,
            spend = text.money(self.after.spend, currency),
            spend_before = text.money(self.before.spend, currency),
            results_name = self.results_name(text),
            results = results(&self.after),
            results_before = results(&self.before),
            cost_name = self.cost_name(text),
            cost = cost(&self.after),
            cost_before = cost(&self.before),
        )
    }

    fn counted(&self, text: &Text, value: f64, fewer: bool) -> Result<String> {
        let value = (value * 10.0).round() / 10.0;
        match (self.by_conversions, fewer) {
            (true, true) => say_amount!(text, follow_up_fewer_conversions, value),
            (false, true) => say_amount!(text, follow_up_fewer_clicks, value),
            (true, false) => say_amount!(text, deep_conversions_n, value),
            (false, false) => say_amount!(text, deep_clicks_n, value),
        }
    }

    fn reason_sentence(&self, text: &Text, currency: &str) -> Result<String> {
        let results_name = self.results_name(text);
        let cost_name = self.cost_name(text);
        let fewer = |before: f64, after: f64| (before - after).max(0.0);
        match self.reason {
            Reason::Confounded { others } => say!(
                text,
                follow_up_confounded,
                span = FOLLOW_UP_DAYS * 2,
                others = text.count(others as f64)
            ),
            Reason::NotServing => Ok(text.written.follow_up_not_serving.to_owned()),
            Reason::FewResults => say!(text, follow_up_few_results, results_name = results_name),
            Reason::WithinVariation => Ok(text.written.follow_up_within_variation.to_owned()),
            Reason::AccountMovedTheSameWay => Ok(text.written.follow_up_account_moved.to_owned()),
            Reason::ExtraSpendBroughtResults => say!(
                text,
                follow_up_extra_spend,
                results_name = results_name,
                cost_name = cost_name
            ),
            Reason::SpendFellMoreThanResults => {
                say!(text, follow_up_spend_fell, results_name = results_name)
            }
            Reason::MovedBetter => say!(
                text,
                follow_up_better,
                cost_name = cost_name,
                results_name = results_name
            ),
            Reason::MovedWorse => say!(
                text,
                follow_up_worse,
                cost_name = cost_name,
                results_name = results_name
            ),
            Reason::Paused => say!(
                text,
                follow_up_paused,
                days = FOLLOW_UP_DAYS,
                spend_less = text.money(fewer(self.before.spend, self.after.spend), currency),
                fewer = self.counted(
                    text,
                    fewer(
                        results(&self.before, self.by_conversions),
                        results(&self.after, self.by_conversions)
                    ),
                    true
                )?
            ),
            Reason::Enabled => say!(
                text,
                follow_up_enabled,
                days = FOLLOW_UP_DAYS,
                spend = text.money(self.after.spend, currency),
                results = self.counted(text, results(&self.after, self.by_conversions), false)?
            ),
        }
    }

    pub fn sentence(&self, text: &Text, currency: &str) -> Result<String> {
        let mut parts = Vec::new();
        parts.extend(self.verdict_sentence(text).map(str::to_owned));
        if self.verdict != Verdict::Reported {
            parts.push(say!(text, follow_up_period, days = FOLLOW_UP_DAYS)?);
            parts.push(self.figures(text, currency)?);
        }
        parts.push(self.reason_sentence(text, currency)?);
        Ok(parts.join(" "))
    }

    fn day_text(&self) -> String {
        crate::workspace::history::date_of(self.change_day)
            .map(|date| date.format("%-d %B").to_string())
            .unwrap_or_default()
    }

    /// The follow-up in code-written English, in the pieces that a text that is too long drops
    /// whole.
    pub fn sentences(&self, currency: &str) -> Vec<String> {
        let text = english();
        let written = "the English follow-up texts have their placeholders";
        let head = format!("{} on {}:", self.change, self.day_text());
        let with_verdict = |verdict: &str, rest: String| vec![format!("{head} {verdict}"), rest];
        match self.verdict {
            Verdict::Worked | Verdict::NotYet | Verdict::NoClearEffect => with_verdict(
                self.verdict_sentence(text).unwrap_or_default(),
                self.figures(text, currency).expect(written),
            ),
            Verdict::CannotBeJudged => with_verdict(
                self.verdict_sentence(text).unwrap_or_default(),
                self.reason_sentence(text, currency).expect(written),
            ),
            Verdict::Reported => vec![format!(
                "{head} {}",
                self.reason_sentence(text, currency).expect(written)
            )],
        }
    }

    pub fn line(&self, currency: &str) -> String {
        self.sentences(currency).join(" ")
    }

    fn fact(&self, currency: &str) -> Value {
        let text = english();
        let cost_label = if self.by_conversions {
            text.written.metric_cost_per_conversion
        } else {
            text.written.metric_cost_per_click
        };
        let shown = |metrics: &Metrics| {
            let mut shown = metrics::formatted(metrics, currency);
            if let Some(value) = cost(metrics, self.by_conversions) {
                shown[cost_label] = metrics::english_money(value, currency).into();
            }
            shown
        };
        let mut changes = json!({
            "Spend": metrics::english_change(self.after.spend, self.before.spend),
            "Clicks": metrics::english_change(self.after.clicks, self.before.clicks),
        });
        if let Some((after, before)) = self.after.conversions.zip(self.before.conversions) {
            changes["Conversions"] = metrics::english_change(after, before).into();
        }
        json!({
            "account_change": format!("{} on {}", self.change, self.day_text()),
            "compared": "the 14 complete days after the change against the 14 complete days before it",
            "before": shown(&self.before),
            "after": shown(&self.after),
            "changes": changes,
            "verdict": self.verdict_sentence(text).unwrap_or("none"),
            "summary": self.sentence(text, currency).expect("the English follow-up texts have their placeholders"),
        })
    }
}

pub fn page_text(text: &Text, follow_up: &FollowUp, currency: &str) -> Result<String> {
    match &follow_up.checked {
        Some(checked) => checked.sentence(text, currency),
        None => say!(
            text,
            follow_up_due,
            date = super::view::day_month(text, follow_up.due_day)?
        ),
    }
}

pub fn facts(checked: &[Checked], currency: &str) -> Value {
    Value::Array(checked.iter().map(|one| one.fact(currency)).collect())
}

pub fn sentences(campaign: &CampaignMovement, currency: &str) -> Vec<String> {
    campaign
        .follow_ups
        .iter()
        .flat_map(|checked| checked.sentences(currency))
        .collect()
}

pub fn only_follow_ups(detection: &Detection) -> bool {
    !detection.campaigns.is_empty()
        && detection.campaigns.iter().all(|campaign| {
            campaign.movements.is_empty()
                && !campaign.follow_ups.is_empty()
                && campaign
                    .reasons
                    .iter()
                    .all(|reason| reason == FOLLOW_UP_DUE)
        })
}

pub fn add_instructions(payload: &mut Value, detection: &Detection) {
    if detection
        .campaigns
        .iter()
        .all(|campaign| campaign.follow_ups.is_empty())
    {
        return;
    }
    if let Some(instructions) = payload["instructions"].as_str() {
        payload["instructions"] = format!("{instructions} {INSTRUCTIONS}").into();
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Claim {
    Positive,
    Negative,
    Neutral,
}

fn claim_of(verdict: Verdict) -> Option<Claim> {
    match verdict {
        Verdict::Worked => Some(Claim::Positive),
        Verdict::NotYet => Some(Claim::Negative),
        Verdict::NoClearEffect | Verdict::CannotBeJudged => Some(Claim::Neutral),
        Verdict::Reported => None,
    }
}

/// The verdicts that a text claims, read from whole words. Contractions are opened and "yet"
/// is dropped, so that "hasn't worked yet" and "has not yet worked" read the same.
fn claims(text: &str) -> Result<Vec<Claim>, ()> {
    let lower = text.to_lowercase().replace("n't", " not");
    let words = lower
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty() && *word != "yet")
        .collect::<Vec<_>>()
        .join(" ");
    let mut words = format!(" {words} ");
    let holds = |words: &str, phrases: &[&str]| {
        phrases
            .iter()
            .any(|phrase| words.contains(&format!(" {phrase} ")))
    };
    if holds(&words, &UNGIVEN_CLAIMS) {
        return Err(());
    }
    let mut found = Vec::new();
    if holds(&words, &NEGATIVE_CLAIMS) {
        found.push(Claim::Negative);
        for phrase in NEGATIVE_CLAIMS {
            words = words.replace(&format!(" {phrase} "), " ");
        }
    }
    if holds(&words, &POSITIVE_CLAIMS) {
        found.push(Claim::Positive);
    }
    if holds(&words, &NEUTRAL_CLAIMS) {
        found.push(Claim::Neutral);
    }
    Ok(found)
}

/// A text keeps the verdicts when every verdict that it claims is one that the code gave. With
/// verdicts of different kinds, the text can show that it keeps them only by claiming none.
pub fn keeps_verdict<'a>(text: &str, checked: impl IntoIterator<Item = &'a Checked>) -> bool {
    let checked = checked.into_iter().collect::<Vec<_>>();
    if checked.is_empty() {
        return true;
    }
    let given = checked
        .iter()
        .filter_map(|one| claim_of(one.verdict))
        .collect::<Vec<_>>();
    let one_kind = given.windows(2).all(|pair| pair[0] == pair[1]);
    claims(text).is_ok_and(|claimed| {
        claimed
            .iter()
            .all(|claim| one_kind && given.contains(claim))
    })
}

pub fn attach(
    detection: &mut Detection,
    days: &[CampaignDay],
    campaigns: &[Campaign],
    changes: &[AccountChange],
    rules: impl Rules,
) {
    let names = campaigns
        .iter()
        .map(|campaign| (campaign.id, campaign.name.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut due: BTreeMap<i64, Vec<Checked>> = BTreeMap::new();
    for found in follow_ups(
        changes,
        days,
        &names,
        &rules,
        &detection.currency,
        detection.day,
    ) {
        if let Some(checked) = found.checked.filter(|_| found.due_day == detection.day) {
            due.entry(found.campaign_id).or_default().push(checked);
        }
    }
    let grouped = by_campaign(days);
    for (id, checked) in due {
        if let Some(movement) = detection
            .campaigns
            .iter_mut()
            .find(|movement| movement.campaign_id == id)
        {
            movement.follow_ups = checked;
        } else if let Some(name) = names.get(&id) {
            let own = grouped.get(&id).map(Vec::as_slice).unwrap_or_default();
            detection.campaigns.push(CampaignMovement {
                campaign_id: id,
                name: name.clone(),
                metrics: lagged_window(own, detection.day - 1, rules.campaign_judged(id)),
                movements: Vec::new(),
                reasons: vec![FOLLOW_UP_DUE.into()],
                account_changes: Vec::new(),
                follow_ups: checked,
            });
        }
    }
    detection
        .campaigns
        .sort_by_key(|campaign| campaign.campaign_id);
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::*;
    use crate::monitoring::{
        campaign_settings::AllFollowAccount, detection::Movement, metrics::CampaignDay,
    };
    use crate::workspace::history::date_of;

    const CHANGE: i64 = 20_000;
    const CAMPAIGN: i64 = 701;

    fn names() -> BTreeMap<i64, String> {
        BTreeMap::from([(CAMPAIGN, "Search - Real".into()), (702, "Display".into())])
    }

    fn change(day: i64, campaign: Option<i64>, old: Value, new: Value) -> AccountChange {
        AccountChange {
            changed_at: format!("{} 10:00:00", date_of(day).unwrap()),
            resource_name: format!("customers/1/changeEvents/{day}-{old}"),
            resource_type: "CAMPAIGN".into(),
            operation: "UPDATE".into(),
            campaign_id: campaign,
            changed_fields: String::new(),
            old_value: old.to_string(),
            new_value: new.to_string(),
        }
    }

    fn budget(day: i64, from: u32, to: u32) -> AccountChange {
        let micros = |dollars: u32| (u64::from(dollars) * 1_000_000).to_string();
        change(
            day,
            Some(CAMPAIGN),
            json!({"campaignBudget":{"amountMicros":micros(from)}}),
            json!({"campaignBudget":{"amountMicros":micros(to)}}),
        )
    }

    fn status(day: i64, from: &str, to: &str) -> AccountChange {
        change(
            day,
            Some(CAMPAIGN),
            json!({"campaign":{"status":from}}),
            json!({"campaign":{"status":to}}),
        )
    }

    fn bid_strategy(day: i64) -> AccountChange {
        change(
            day,
            Some(CAMPAIGN),
            json!({"campaign":{"biddingStrategyType":"MAXIMIZE_CLICKS"}}),
            json!({"campaign":{"biddingStrategyType":"TARGET_CPA"}}),
        )
    }

    fn target(day: i64, from: u32, to: u32) -> AccountChange {
        let micros = |dollars: u32| (u64::from(dollars) * 1_000_000).to_string();
        change(
            day,
            Some(CAMPAIGN),
            json!({"campaign":{"targetCpa":{"targetCpaMicros":micros(from)}}}),
            json!({"campaign":{"targetCpa":{"targetCpaMicros":micros(to)}}}),
        )
    }

    #[derive(Clone, Copy)]
    struct Totals {
        spend: f64,
        clicks: f64,
        conversions: f64,
    }

    fn totals(spend: f64, clicks: f64, conversions: f64) -> Totals {
        Totals {
            spend,
            clicks,
            conversions,
        }
    }

    fn spread(rows: &mut Vec<CampaignDay>, from: i64, length: i64, total: Totals) {
        let share = length as f64;
        for day in from..from + length {
            rows.push(CampaignDay {
                day,
                campaign_id: CAMPAIGN,
                metrics: Metrics {
                    spend: total.spend / share,
                    clicks: total.clicks / share,
                    impressions: total.clicks * 10.0 / share,
                    conversions: Some(total.conversions / share),
                    conversion_value: Some(0.0),
                },
            });
        }
    }

    fn spread_of(rows: &mut Vec<CampaignDay>, campaign_id: i64, from: i64, total: Totals) {
        let mut own = Vec::new();
        spread(&mut own, from, 14, total);
        rows.extend(
            own.into_iter()
                .map(|row| CampaignDay { campaign_id, ..row }),
        );
    }

    fn with_rest_of_account(
        mut rows: Vec<CampaignDay>,
        before: Totals,
        after: Totals,
    ) -> Vec<CampaignDay> {
        spread_of(&mut rows, 702, CHANGE - 14, before);
        spread_of(&mut rows, 702, CHANGE + 1, after);
        rows
    }

    fn keyword(day: i64, campaign: Option<i64>) -> AccountChange {
        let mut change = change(
            day,
            campaign,
            json!({"adGroupCriterion":{"keyword":{"text":"old"}}}),
            json!({"adGroupCriterion":{"keyword":{"text":"new"}}}),
        );
        change.resource_type = "AD_GROUP_CRITERION".into();
        change
    }

    fn history(earlier: [Totals; 2], before: Totals, after: Totals) -> Vec<CampaignDay> {
        let mut rows = Vec::new();
        spread(&mut rows, CHANGE - 42, 14, earlier[0]);
        spread(&mut rows, CHANGE - 28, 14, earlier[1]);
        spread(&mut rows, CHANGE - 14, 14, before);
        spread(&mut rows, CHANGE, 1, totals(9999.0, 9999.0, 999.0));
        spread(&mut rows, CHANGE + 1, 14, after);
        let daily = |value: f64| value / 14.0;
        let lagging = totals(
            daily(after.spend),
            daily(after.clicks),
            daily(after.conversions),
        );
        spread(&mut rows, CHANGE + 15, 1, lagging);
        rows
    }

    fn flat(before: Totals, after: Totals) -> Vec<CampaignDay> {
        history([before, before], before, after)
    }

    fn checked_at(
        changes: &[AccountChange],
        days: &[CampaignDay],
        judged: bool,
        day: i64,
    ) -> Vec<FollowUp> {
        follow_ups(
            changes,
            days,
            &names(),
            AllFollowAccount { judged },
            "USD",
            day,
        )
    }

    fn result(changes: &[AccountChange], days: &[CampaignDay], judged: bool) -> Checked {
        checked_at(changes, days, judged, due_day(CHANGE))
            .into_iter()
            .find(|follow_up| follow_up.change_day == CHANGE)
            .and_then(|follow_up| follow_up.checked)
            .expect("the follow-up is due")
    }

    fn verdict(changes: &[AccountChange], days: &[CampaignDay]) -> (Verdict, Reason) {
        let checked = result(changes, days, true);
        (checked.verdict, checked.reason)
    }

    #[test]
    fn a_follow_up_is_due_on_the_first_day_that_its_conversions_are_complete() {
        assert_eq!(due_day(CHANGE), CHANGE + 16);
        let changes = [budget(CHANGE, 50, 70)];
        let days = flat(totals(700.0, 140.0, 14.0), totals(980.0, 196.0, 21.0));
        let early = checked_at(&changes, &days, true, CHANGE + 15);
        assert_eq!(early.len(), 1);
        assert_eq!(early[0].due_day, CHANGE + 16);
        assert!(early[0].checked.is_none());
        let due = checked_at(&changes, &days, true, CHANGE + 16);
        assert_eq!(due[0].change, "Daily budget changed from $50.00 to $70.00");
        assert_eq!(due[0].change_day, CHANGE);
        assert_eq!(due[0].campaign_id, CAMPAIGN);
        assert!(due[0].checked.is_some());
    }

    #[test]
    fn the_day_of_the_change_and_the_lagging_days_belong_to_neither_period() {
        let changes = [budget(CHANGE, 50, 70)];
        let mut days = flat(totals(700.0, 140.0, 14.0), totals(980.0, 196.0, 21.0));
        spread(&mut days, CHANGE + 15, 1, totals(5000.0, 5000.0, 500.0));
        let checked = result(&changes, &days, true);
        assert_eq!(checked.before.spend, 700.0);
        assert_eq!(checked.before.conversions, Some(14.0));
        assert_eq!(checked.after.spend, 980.0);
        assert_eq!(checked.after.conversions, Some(21.0));
    }

    #[test]
    fn a_raised_budget_that_brought_conversions_at_a_similar_cost_worked() {
        let days = flat(totals(700.0, 140.0, 70.0), totals(980.0, 196.0, 98.0));
        assert_eq!(
            verdict(&[budget(CHANGE, 50, 70)], &days),
            (Verdict::Worked, Reason::ExtraSpendBroughtResults)
        );
    }

    #[test]
    fn a_raised_budget_that_made_each_conversion_dearer_has_not_worked_yet() {
        let days = flat(totals(700.0, 140.0, 14.0), totals(980.0, 196.0, 14.0));
        assert_eq!(
            verdict(&[budget(CHANGE, 50, 70)], &days),
            (Verdict::NotYet, Reason::MovedWorse)
        );
    }

    #[test]
    fn a_raised_budget_with_a_small_difference_has_no_clear_effect() {
        let days = flat(totals(700.0, 140.0, 14.0), totals(740.0, 148.0, 14.0));
        assert_eq!(
            verdict(&[budget(CHANGE, 50, 70)], &days),
            (Verdict::NoClearEffect, Reason::WithinVariation)
        );
    }

    #[test]
    fn a_lowered_budget_that_lost_fewer_conversions_than_spend_worked() {
        let days = flat(totals(980.0, 196.0, 21.0), totals(700.0, 140.0, 20.0));
        assert_eq!(
            verdict(&[budget(CHANGE, 70, 50)], &days),
            (Verdict::Worked, Reason::SpendFellMoreThanResults)
        );
    }

    #[test]
    fn a_lowered_budget_that_lost_conversions_as_fast_as_spend_has_not_worked_yet() {
        let days = flat(totals(980.0, 196.0, 210.0), totals(700.0, 140.0, 140.0));
        assert_eq!(
            verdict(&[budget(CHANGE, 70, 50)], &days),
            (Verdict::NotYet, Reason::MovedWorse)
        );
    }

    #[test]
    fn a_lowered_budget_with_a_small_difference_has_no_clear_effect() {
        let days = flat(totals(980.0, 196.0, 21.0), totals(960.0, 192.0, 21.0));
        assert_eq!(
            verdict(&[budget(CHANGE, 70, 68)], &days),
            (Verdict::NoClearEffect, Reason::WithinVariation)
        );
    }

    #[test]
    fn a_bid_strategy_that_lowered_the_cost_and_kept_the_conversions_worked() {
        let days = flat(totals(700.0, 140.0, 14.0), totals(700.0, 140.0, 21.0));
        assert_eq!(
            verdict(&[bid_strategy(CHANGE)], &days),
            (Verdict::Worked, Reason::MovedBetter)
        );
    }

    #[test]
    fn a_target_that_raised_the_cost_and_lost_conversions_has_not_worked_yet() {
        let days = flat(totals(700.0, 140.0, 14.0), totals(700.0, 140.0, 7.0));
        assert_eq!(
            verdict(&[target(CHANGE, 40, 60)], &days),
            (Verdict::NotYet, Reason::MovedWorse)
        );
    }

    #[test]
    fn a_target_that_moved_the_cost_by_less_than_the_floor_has_no_clear_effect() {
        let days = flat(totals(700.0, 140.0, 14.0), totals(700.0, 140.0, 14.2));
        assert_eq!(
            verdict(&[target(CHANGE, 40, 60)], &days),
            (Verdict::NoClearEffect, Reason::WithinVariation)
        );
    }

    #[test]
    fn a_difference_inside_the_normal_variation_of_the_campaign_has_no_clear_effect() {
        let days = history(
            [totals(700.0, 140.0, 40.0), totals(700.0, 140.0, 12.0)],
            totals(700.0, 140.0, 14.0),
            totals(700.0, 140.0, 21.0),
        );
        assert_eq!(
            verdict(&[bid_strategy(CHANGE)], &days),
            (Verdict::NoClearEffect, Reason::WithinVariation)
        );
    }

    #[test]
    fn a_campaign_that_is_not_judged_by_conversions_is_followed_up_by_clicks() {
        let days = flat(totals(700.0, 140.0, 0.0), totals(980.0, 196.0, 0.0));
        let checked = result(&[budget(CHANGE, 50, 70)], &days, false);
        assert_eq!(
            (checked.verdict, checked.reason),
            (Verdict::Worked, Reason::ExtraSpendBroughtResults)
        );
        assert!(!checked.by_conversions);
        assert_eq!(checked.before.conversions, None);
        assert_eq!(checked.after.conversions, None);
        assert_eq!(checked.after.clicks, 196.0);
    }

    #[test]
    fn a_pause_and_a_restart_state_the_numbers_and_give_no_verdict() {
        let paused = flat(totals(700.0, 140.0, 14.0), totals(0.0, 0.0, 0.0));
        let checked = result(&[status(CHANGE, "ENABLED", "PAUSED")], &paused, true);
        assert_eq!(
            (checked.kind, checked.verdict, checked.reason),
            (Kind::Paused, Verdict::Reported, Reason::Paused)
        );
        assert_eq!(checked.before.spend, 700.0);
        assert_eq!(checked.after.spend, 0.0);
        let enabled = flat(totals(0.0, 0.0, 0.0), totals(700.0, 140.0, 14.0));
        let checked = result(&[status(CHANGE, "PAUSED", "ENABLED")], &enabled, true);
        assert_eq!(
            (checked.kind, checked.verdict, checked.reason),
            (Kind::Enabled, Verdict::Reported, Reason::Enabled)
        );
        assert_eq!(checked.after.conversions, Some(14.0));
    }

    #[test]
    fn any_other_change_of_the_campaign_or_of_no_campaign_in_the_same_28_days_blocks_the_verdict() {
        let days = flat(totals(700.0, 140.0, 70.0), totals(980.0, 196.0, 98.0));
        let mut display = bid_strategy(CHANGE + 3);
        display.campaign_id = Some(702);
        let changes = [
            budget(CHANGE, 50, 70),
            keyword(CHANGE + 3, Some(CAMPAIGN)),
            keyword(CHANGE - 10, None),
            display,
            keyword(CHANGE - 15, Some(CAMPAIGN)),
            keyword(CHANGE + 15, Some(CAMPAIGN)),
        ];
        assert_eq!(
            verdict(&changes, &days),
            (Verdict::CannotBeJudged, Reason::Confounded { others: 2 })
        );
    }

    #[test]
    fn a_campaign_paused_and_enabled_again_the_next_day_has_no_sentence_about_savings() {
        let days = flat(totals(700.0, 140.0, 14.0), totals(0.0, 0.0, 0.0));
        let changes = [
            status(CHANGE, "ENABLED", "PAUSED"),
            status(CHANGE + 1, "PAUSED", "ENABLED"),
        ];
        let checked = result(&changes, &days, true);
        assert_eq!(
            (checked.kind, checked.verdict, checked.reason),
            (
                Kind::Paused,
                Verdict::CannotBeJudged,
                Reason::Confounded { others: 1 }
            )
        );
        assert!(!checked
            .sentence(english(), "USD")
            .unwrap()
            .contains("spent"));
    }

    #[test]
    fn a_campaign_that_did_not_serve_on_most_days_cannot_be_judged() {
        let mut days = Vec::new();
        let steady = totals(700.0, 140.0, 14.0);
        for from in [CHANGE - 42, CHANGE - 28, CHANGE - 14] {
            spread(&mut days, from, 14, steady);
        }
        spread(&mut days, CHANGE + 1, 6, totals(980.0, 196.0, 21.0));
        assert_eq!(
            verdict(&[budget(CHANGE, 50, 70)], &days),
            (Verdict::CannotBeJudged, Reason::NotServing)
        );
    }

    #[test]
    fn too_few_conversions_or_clicks_cannot_be_judged() {
        let few_conversions = flat(totals(700.0, 140.0, 3.0), totals(980.0, 196.0, 21.0));
        assert_eq!(
            verdict(&[budget(CHANGE, 50, 70)], &few_conversions),
            (Verdict::CannotBeJudged, Reason::FewResults)
        );
        let few_clicks = flat(totals(70.0, 60.0, 0.0), totals(98.0, 90.0, 0.0));
        let checked = result(&[budget(CHANGE, 50, 70)], &few_clicks, false);
        assert_eq!(
            (checked.verdict, checked.reason),
            (Verdict::CannotBeJudged, Reason::FewResults)
        );
    }

    #[test]
    fn a_change_younger_than_its_due_day_shows_when_the_follow_up_comes() {
        let days = flat(totals(700.0, 140.0, 14.0), totals(980.0, 196.0, 21.0));
        let found = checked_at(&[budget(CHANGE, 50, 70)], &days, true, CHANGE + 3);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].due_day, CHANGE + 16);
        assert_eq!(found[0].change_day, CHANGE);
        assert!(found[0].checked.is_none());
    }

    #[test]
    fn a_change_that_is_unreadable_or_has_no_campaign_or_no_direction_has_no_follow_up() {
        let days = flat(totals(700.0, 140.0, 14.0), totals(980.0, 196.0, 21.0));
        let mut without_campaign = budget(CHANGE, 50, 70);
        without_campaign.campaign_id = None;
        let unreadable = change(
            CHANGE,
            Some(CAMPAIGN),
            json!({"campaign":{"name":"A"}}),
            json!({"campaign":{"name":"B"}}),
        );
        let changes = [
            without_campaign,
            unreadable,
            budget(CHANGE, 50, 50),
            status(CHANGE, "ENABLED", "REMOVED"),
        ];
        assert!(checked_at(&changes, &days, true, due_day(CHANGE)).is_empty());
    }

    #[test]
    fn a_target_cost_change_of_a_campaign_not_judged_by_conversions_has_no_follow_up() {
        let days = flat(totals(700.0, 140.0, 0.0), totals(700.0, 140.0, 0.0));
        assert!(checked_at(&[target(CHANGE, 40, 60)], &days, false, due_day(CHANGE)).is_empty());
    }

    #[test]
    fn the_follow_ups_are_listed_oldest_change_first() {
        let days = flat(totals(700.0, 140.0, 14.0), totals(980.0, 196.0, 21.0));
        let found = checked_at(
            &[budget(CHANGE + 2, 50, 70), budget(CHANGE - 2, 40, 50)],
            &days,
            true,
            CHANGE + 40,
        );
        assert_eq!(
            found.iter().map(|f| f.change_day).collect::<Vec<_>>(),
            [CHANGE - 2, CHANGE + 2]
        );
    }

    fn raised() -> (Vec<AccountChange>, Vec<CampaignDay>) {
        (
            vec![budget(CHANGE, 50, 70)],
            flat(totals(700.0, 140.0, 70.0), totals(980.0, 196.0, 98.0)),
        )
    }

    #[test]
    fn the_sentence_states_the_verdict_the_two_periods_and_the_reason() {
        let (changes, days) = raised();
        let checked = result(&changes, &days, true);
        assert_eq!(
            checked.sentence(english(), "USD").unwrap(),
            "It worked. In the 14 days after the change, against the 14 days before it. Spend $980 (before $700), conversions 98 (before 70), cost per conversion $10.00 (before $10.00). The added spend brought more conversions at a similar cost per conversion."
        );
    }

    #[test]
    fn a_campaign_judged_by_clicks_never_mentions_conversions() {
        let days = flat(totals(700.0, 140.0, 0.0), totals(980.0, 196.0, 0.0));
        let checked = result(&[budget(CHANGE, 50, 70)], &days, false);
        let sentence = checked.sentence(english(), "USD").unwrap();
        assert_eq!(
            sentence,
            "It worked. In the 14 days after the change, against the 14 days before it. Spend $980 (before $700), clicks 196 (before 140), cost per click $5.00 (before $5.00). The added spend brought more clicks at a similar cost per click."
        );
        assert!(!checked.line("USD").to_lowercase().contains("conversion"));
        assert!(!serde_json::to_string(&checked.fact("USD"))
            .unwrap()
            .to_lowercase()
            .contains("conversion"));
    }

    #[test]
    fn a_pause_states_what_the_ad_account_saved_and_lost() {
        let days = flat(totals(700.0, 140.0, 14.0), totals(0.0, 0.0, 0.0));
        let checked = result(&[status(CHANGE, "ENABLED", "PAUSED")], &days, true);
        assert_eq!(
            checked.sentence(english(), "USD").unwrap(),
            "In the 14 days after the Campaign was paused, the Campaign spent $700 less and got 14 fewer conversions."
        );
        let days = flat(totals(0.0, 0.0, 0.0), totals(700.0, 140.0, 14.0));
        let checked = result(&[status(CHANGE, "PAUSED", "ENABLED")], &days, true);
        assert_eq!(
            checked.sentence(english(), "USD").unwrap(),
            "In the 14 days after the Campaign was enabled, it spent $700 and got 14 conversions."
        );
    }

    #[test]
    fn a_confounded_change_names_how_many_other_changes_there_were() {
        let days = flat(totals(700.0, 140.0, 14.0), totals(980.0, 196.0, 21.0));
        let checked = result(
            &[budget(CHANGE, 50, 70), bid_strategy(CHANGE + 5)],
            &days,
            true,
        );
        assert_eq!(
            checked.line("USD"),
            "Daily budget changed from $50.00 to $70.00 on 4 October: KeySend cannot judge it. Other changes to this Campaign in the same 28 days: 1. The effect cannot be separated."
        );
    }

    #[test]
    fn no_english_follow_up_text_claims_a_cause() {
        let causes = [
            "because",
            "due to",
            "caused by",
            "thanks to",
            "as a result of",
            "owing to",
            "led to",
            "driven by",
        ];
        let kinds = [
            (budget(CHANGE, 50, 70), totals(980.0, 196.0, 21.0)),
            (budget(CHANGE, 50, 70), totals(980.0, 196.0, 14.0)),
            (budget(CHANGE, 70, 50), totals(700.0, 140.0, 20.0)),
            (bid_strategy(CHANGE), totals(700.0, 140.0, 7.0)),
            (status(CHANGE, "ENABLED", "PAUSED"), totals(0.0, 0.0, 0.0)),
        ];
        for (change, after) in kinds {
            let days = flat(totals(980.0, 196.0, 21.0), after);
            let checked = result(&[change], &days, true);
            for text in [
                checked.sentence(english(), "USD").unwrap(),
                checked.line("USD"),
            ] {
                let words = text
                    .to_lowercase()
                    .split(|character: char| !character.is_alphanumeric())
                    .filter(|word| !word.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                let words = format!(" {words} ");
                assert!(
                    causes
                        .iter()
                        .all(|cause| !words.contains(&format!(" {cause} "))),
                    "{text}"
                );
            }
        }
    }

    fn paused_after(before_conversions: f64) -> Checked {
        let days = flat(
            totals(700.0, 140.0, before_conversions),
            totals(0.0, 0.0, 0.0),
        );
        result(&[status(CHANGE, "ENABLED", "PAUSED")], &days, true)
    }

    #[test]
    fn every_language_states_what_a_paused_campaign_saved_and_lost_in_its_own_words() {
        use crate::language::{de, en, es, fr, it, pt, ro};
        let checked = paused_after(21.0);
        for (text, sentence) in [
            (&en::TEXT, "In the 14 days after the Campaign was paused, the Campaign spent $700 less and got 21 fewer conversions."),
            (&de::TEXT, "In den 14 Tagen nach der Pausierung der Kampagne gab die Kampagne 700\u{a0}$ weniger aus und erzielte 21 Conversions weniger."),
            (&es::TEXT, "En los 14 días posteriores a la pausa de la campaña, la campaña gastó 700\u{a0}$ menos y obtuvo 21 conversiones menos."),
            (&fr::TEXT, "Pendant les 14 jours après la mise en pause de la campagne, la campagne a dépensé 700\u{a0}$ de moins et obtenu 21 conversions de moins."),
            (&it::TEXT, "Nei 14 giorni dopo la messa in pausa della campagna, la campagna ha speso 700 $ in meno e ottenuto 21 conversioni in meno."),
            (&pt::TEXT, "Nos 14 dias depois da pausa da campanha, a campanha gastou US$ 700 a menos e obteve 21 conversões a menos."),
            (&ro::TEXT, "În cele 14 zile de după oprirea campaniei, campania a cheltuit cu 700\u{a0}$ mai puțin și a obținut 21 de conversii în minus."),
        ] {
            assert_eq!(checked.sentence(text, "USD").unwrap(), sentence);
        }
    }

    #[test]
    fn a_count_of_one_and_a_count_of_twenty_one_take_their_own_plural_form() {
        use crate::language::{en, ro};
        let one = paused_after(1.0);
        assert_eq!(
            one.sentence(&en::TEXT, "USD").unwrap(),
            "In the 14 days after the Campaign was paused, the Campaign spent $700 less and got 1 fewer conversion."
        );
        assert_eq!(
            one.sentence(&ro::TEXT, "USD").unwrap(),
            "În cele 14 zile de după oprirea campaniei, campania a cheltuit cu 700\u{a0}$ mai puțin și a obținut 1 conversie în minus."
        );
        let days = flat(totals(0.0, 0.0, 0.0), totals(700.0, 140.0, 21.0));
        let enabled = result(&[status(CHANGE, "PAUSED", "ENABLED")], &days, true);
        assert_eq!(
            enabled.sentence(&ro::TEXT, "USD").unwrap(),
            "În cele 14 zile de după activarea campaniei, aceasta a cheltuit 700\u{a0}$ și a obținut 21 de conversii."
        );
    }

    #[test]
    fn a_rise_of_a_few_conversions_is_not_beyond_the_normal_variation_of_a_count() {
        let few = flat(totals(700.0, 140.0, 6.0), totals(980.0, 196.0, 10.0));
        assert_eq!(
            verdict(&[budget(CHANGE, 50, 70)], &few),
            (Verdict::NoClearEffect, Reason::WithinVariation)
        );
        let many = flat(totals(700.0, 140.0, 100.0), totals(980.0, 196.0, 150.0));
        assert_eq!(
            verdict(&[budget(CHANGE, 50, 70)], &many),
            (Verdict::Worked, Reason::ExtraSpendBroughtResults)
        );
    }

    #[test]
    fn a_cost_that_moves_by_less_than_a_tenth_of_the_cost_before_is_no_clear_effect() {
        for (before, after) in [
            (totals(700.0, 140.0, 14.0), totals(700.0, 140.0, 15.0)),
            (totals(70_000.0, 140.0, 14.0), totals(69_000.0, 140.0, 14.0)),
        ] {
            assert_eq!(
                verdict(&[bid_strategy(CHANGE)], &flat(before, after)),
                (Verdict::NoClearEffect, Reason::WithinVariation)
            );
        }
        let clear = flat(totals(700.0, 140.0, 14.0), totals(700.0, 140.0, 20.0));
        assert_eq!(
            verdict(&[bid_strategy(CHANGE)], &clear),
            (Verdict::Worked, Reason::MovedBetter)
        );
    }

    fn raised_while_the_rest_moved(rest_before: f64, rest_after: f64) -> (Verdict, Reason) {
        let own = flat(totals(700.0, 140.0, 70.0), totals(980.0, 196.0, 98.0));
        let days = with_rest_of_account(
            own,
            totals(rest_before * 10.0, rest_before * 2.0, rest_before),
            totals(rest_after * 10.0, rest_after * 2.0, rest_after),
        );
        verdict(&[budget(CHANGE, 50, 70)], &days)
    }

    #[test]
    fn a_raise_that_the_rest_of_the_ad_account_matched_has_no_clear_effect() {
        assert_eq!(
            raised_while_the_rest_moved(700.0, 910.0),
            (Verdict::NoClearEffect, Reason::AccountMovedTheSameWay)
        );
        assert_eq!(
            raised_while_the_rest_moved(700.0, 875.0),
            (Verdict::NoClearEffect, Reason::AccountMovedTheSameWay),
            "a rise of 25% elsewhere is more than half of the 40% here"
        );
    }

    #[test]
    fn a_rest_of_the_ad_account_that_moved_less_or_the_other_way_leaves_the_verdict() {
        let worked = (Verdict::Worked, Reason::ExtraSpendBroughtResults);
        for (before, after) in [
            (700.0, 805.0),
            (700.0, 700.0),
            (700.0, 490.0),
            (700.0, 0.0),
            (0.0, 0.0),
        ] {
            assert_eq!(
                raised_while_the_rest_moved(before, after),
                worked,
                "{before} {after}"
            );
        }
    }

    #[test]
    fn a_small_count_elsewhere_is_no_season() {
        assert_eq!(
            raised_while_the_rest_moved(14.0, 18.0),
            (Verdict::Worked, Reason::ExtraSpendBroughtResults)
        );
    }

    #[test]
    fn a_campaign_alone_in_its_ad_account_has_no_season_to_compare() {
        let (changes, days) = raised();
        assert_eq!(
            verdict(&changes, &days),
            (Verdict::Worked, Reason::ExtraSpendBroughtResults)
        );
    }

    #[test]
    fn the_sentence_for_a_season_says_the_whole_ad_account_moved_the_same_way() {
        let own = flat(totals(700.0, 140.0, 70.0), totals(980.0, 196.0, 98.0));
        let days = with_rest_of_account(
            own,
            totals(7000.0, 1400.0, 700.0),
            totals(9100.0, 1820.0, 910.0),
        );
        let checked = result(&[budget(CHANGE, 50, 70)], &days, true);
        assert_eq!(
            checked.sentence(english(), "USD").unwrap(),
            "There is no clear effect. In the 14 days after the change, against the 14 days before it. Spend $980 (before $700), conversions 98 (before 70), cost per conversion $10.00 (before $10.00). The rest of the Ad Account moved the same way over the same two periods."
        );
    }

    #[test]
    fn a_summary_cannot_change_a_verdict_that_a_listed_campaign_has() {
        let days = flat(totals(700.0, 140.0, 14.0), totals(980.0, 196.0, 14.0));
        let detection = detected(&[budget(CHANGE, 50, 70)], &days, CHANGE + 15);
        assert_eq!(
            detection.campaigns[0].follow_ups[0].verdict,
            Verdict::NotYet
        );
        let answer = |summary: &str| {
            crate::monitoring::update::accept(
                json!({"summary":summary,"lines":[{"campaign_id":701,"text":"Spend rose after the daily budget change."}]}),
                &detection,
            )
        };
        assert!(answer("The budget raise worked.").is_err());
        assert!(answer("The budget raise has not worked yet.").is_ok());
        assert!(answer("Spend rose after the budget raise.").is_ok());
    }

    fn movement_of(metric: MetricName, current: f64, previous: f64) -> Movement {
        Movement {
            metric,
            current,
            previous,
            normal_variation: 1.0,
        }
    }

    fn followed_with_movements(movements: Vec<Movement>) -> Detection {
        let (changes, days) = raised();
        let mut detection = detected(&changes, &days, CHANGE + 15);
        detection.campaigns[0].movements = movements;
        detection.campaigns[0].metrics.current = Metrics {
            spend: 1234.0,
            clicks: 2345.0,
            impressions: 0.0,
            conversions: Some(345.0),
            conversion_value: None,
        };
        detection.campaigns[0].metrics.previous = Metrics {
            spend: 567.0,
            clicks: 678.0,
            impressions: 0.0,
            conversions: Some(89.0),
            conversion_value: None,
        };
        detection
    }

    #[test]
    fn a_fallback_line_puts_the_movements_first_and_drops_whole_sentences_that_do_not_fit() {
        let detection = followed_with_movements(vec![
            movement_of(MetricName::Spend, 1234.0, 567.0),
            movement_of(MetricName::Clicks, 2345.0, 678.0),
            movement_of(MetricName::Conversions, 345.0, 89.0),
        ]);
        let line = &crate::monitoring::update::fallback(&detection).lines[0].text;
        assert_eq!(
            line,
            "Spend was $1,234 against $567 in the previous 7 days. Clicks was 2,345 against 678 in the previous 7 days. Conversions was 345 against 89 in the 7 days that ended 2 days ago."
        );
    }

    #[test]
    fn a_fallback_line_keeps_a_whole_follow_up_sentence_that_fits_after_the_movement() {
        let detection = followed_with_movements(vec![
            movement_of(MetricName::Spend, 1234.0, 567.0),
            movement_of(MetricName::Clicks, 2345.0, 678.0),
        ]);
        let line = &crate::monitoring::update::fallback(&detection).lines[0].text;
        assert_eq!(
            line,
            "Spend was $1,234 against $567 in the previous 7 days. Clicks was 2,345 against 678 in the previous 7 days. Daily budget changed from $50.00 to $70.00 on 4 October: It worked."
        );
    }

    fn real_campaign() -> Campaign {
        Campaign {
            id: CAMPAIGN,
            name: "Search - Real".into(),
            status: "ENABLED".into(),
            channel_type: "SEARCH".into(),
            bidding_strategy: "MAXIMIZE_CONVERSIONS".into(),
            budget_micros: Some(70_000_000),
        }
    }

    fn detect_at(days: &[CampaignDay], end: i64) -> Detection {
        crate::monitoring::detection::detect(
            days,
            &[real_campaign()],
            &[],
            end,
            CHANGE - 42,
            AllFollowAccount { judged: true },
            "USD",
        )
    }

    fn detected(changes: &[AccountChange], days: &[CampaignDay], end: i64) -> Detection {
        let mut detection = detect_at(days, end);
        attach(
            &mut detection,
            days,
            &[real_campaign()],
            changes,
            AllFollowAccount { judged: true },
        );
        detection
    }

    #[test]
    fn a_follow_up_gives_its_campaign_a_line_even_when_nothing_else_moved() {
        let steady = totals(700.0, 140.0, 14.0);
        let days = flat(steady, steady);
        assert!(detect_at(&days, CHANGE + 15).quiet());
        let detection = detected(&[budget(CHANGE, 50, 70)], &days, CHANGE + 15);
        assert!(!detection.quiet());
        assert_eq!(detection.day, CHANGE + 16);
        assert_eq!(detection.campaigns.len(), 1);
        assert_eq!(detection.campaigns[0].name, "Search - Real");
        assert_eq!(detection.campaigns[0].reasons, [FOLLOW_UP_DUE]);
        assert_eq!(detection.campaigns[0].follow_ups.len(), 1);
        assert_eq!(
            detection.campaigns[0].follow_ups[0].verdict,
            Verdict::NoClearEffect
        );
    }

    #[test]
    fn a_follow_up_belongs_to_the_update_of_its_due_day_only() {
        let (changes, days) = raised();
        for end in [CHANGE + 14, CHANGE + 16] {
            let detection = detected(&changes, &days, end);
            assert!(
                detection
                    .campaigns
                    .iter()
                    .all(|movement| movement.follow_ups.is_empty()),
                "{end}"
            );
        }
    }

    #[test]
    fn the_follow_up_reaches_the_ai_as_facts_and_the_fallback_states_it_in_code_written_words() {
        let (changes, days) = raised();
        let detection = detected(&changes, &days, CHANGE + 15);
        let facts = crate::monitoring::update::facts(&detection);
        let follow_up = &facts["campaigns"][0]["follow_ups"][0];
        assert_eq!(
            follow_up["account_change"],
            "Daily budget changed from $50.00 to $70.00 on 4 October"
        );
        assert_eq!(follow_up["verdict"], "It worked.");
        assert_eq!(follow_up["after"]["Conversions"], "98");
        assert_eq!(follow_up["before"]["Cost per conversion"], "$10.00");
        assert_eq!(follow_up["after"]["Cost per conversion"], "$10.00");
        let update = crate::monitoring::update::fallback(&detection);
        assert_eq!(
            update.lines[0].text,
            "Daily budget changed from $50.00 to $70.00 on 4 October: It worked. Spend $980 (before $700), conversions 98 (before 70), cost per conversion $10.00 (before $10.00)."
        );
        assert!(update.lines[0].text.chars().count() <= crate::monitoring::update::LINE_LENGTH);
    }

    #[test]
    fn the_ai_may_word_the_follow_up_but_not_change_its_verdict() {
        let (changes, days) = raised();
        let detection = detected(&changes, &days, CHANGE + 15);
        let answer = |line: &str| {
            crate::monitoring::update::accept(
                json!({"summary":"Spend rose after the budget change.","lines":[{"campaign_id":701,"text":line}]}),
                &detection,
            )
        };
        let accepted =
            answer("Conversions rose from 70 to 98 after the daily budget change, and it worked.")
                .unwrap();
        assert_eq!(
            accepted.lines[0].text,
            "Conversions rose from 70 to 98 after the daily budget change, and it worked."
        );
        for line in [
            "Conversions rose from 70 to 98 after the daily budget change, but it has not worked yet.",
            "Conversions rose from 70 to 98 after the daily budget change, with no clear effect.",
            "Conversions rose from 70 to 98 after the daily budget change, so KeySend cannot judge it.",
            "Conversions rose from 70 to 98 after the daily budget change, but it failed.",
        ] {
            assert!(answer(line).is_err(), "{line}");
        }
    }

    fn given(verdict: Verdict) -> Checked {
        let (changes, days) = raised();
        Checked {
            verdict,
            ..result(&changes, &days, true)
        }
    }

    #[test]
    fn a_text_keeps_the_verdict_only_when_it_claims_no_other_verdict() {
        let cases = [
            (Verdict::Worked, "It worked after the budget change.", true),
            (
                Verdict::Worked,
                "It did not work after the budget change.",
                false,
            ),
            (Verdict::Worked, "It has not worked yet.", false),
            (Verdict::Worked, "The raise failed.", false),
            (Verdict::Worked, "The raise is not working.", false),
            (Verdict::Worked, "The raise had no clear effect.", false),
            (Verdict::NotYet, "It has not paid off yet.", true),
            (Verdict::NotYet, "It hasn't worked yet.", true),
            (Verdict::NotYet, "It has not yet worked.", true),
            (Verdict::NotYet, "It paid off.", false),
            (Verdict::NotYet, "It failed.", false),
            (
                Verdict::CannotBeJudged,
                "The effect cannot be separated.",
                true,
            ),
            (Verdict::CannotBeJudged, "KeySend cannot judge it.", true),
            (Verdict::CannotBeJudged, "It worked.", false),
            (Verdict::NoClearEffect, "There is no clear effect.", true),
            (Verdict::NoClearEffect, "It has not worked yet.", false),
            (Verdict::Reported, "Spend fell after the pause.", true),
            (Verdict::Reported, "It worked.", false),
        ];
        for (verdict, text, kept) in cases {
            assert_eq!(
                keeps_verdict(text, [&given(verdict)]),
                kept,
                "{verdict:?} {text}"
            );
        }
    }

    #[test]
    fn two_follow_ups_with_different_verdicts_allow_a_text_that_claims_none() {
        let both = [given(Verdict::Worked), given(Verdict::NotYet)];
        assert!(keeps_verdict("Spend rose after the budget change.", &both));
        assert!(!keeps_verdict("It worked.", &both));
        assert!(!keeps_verdict("It has not worked yet.", &both));
        let same = [given(Verdict::Worked), given(Verdict::Worked)];
        assert!(keeps_verdict("Both changes worked.", &same));
    }

    #[test]
    fn a_text_with_no_follow_up_is_not_checked() {
        assert!(keeps_verdict("It worked.", []));
        assert!(keeps_verdict("It failed.", []));
    }

    #[test]
    fn the_prompt_gains_the_follow_up_sentences_only_when_a_follow_up_is_due() {
        let (changes, days) = raised();
        let with = detected(&changes, &days, CHANGE + 15);
        let mut payload = json!({"instructions": "Base."});
        add_instructions(&mut payload, &with);
        assert_eq!(payload["instructions"], format!("Base. {INSTRUCTIONS}"));
        let without = detected(&[], &days, CHANGE + 15);
        let mut payload = json!({"instructions": "Base."});
        add_instructions(&mut payload, &without);
        assert_eq!(payload["instructions"], "Base.");
    }

    #[test]
    fn an_update_with_only_a_follow_up_says_so() {
        let steady = totals(700.0, 140.0, 14.0);
        let days = flat(steady, steady);
        let detection = detected(&[budget(CHANGE, 50, 70)], &days, CHANGE + 15);
        assert!(only_follow_ups(&detection));
        assert_eq!(
            crate::monitoring::update::fallback(&detection).summary,
            SUMMARY
        );
    }

    #[test]
    fn a_campaign_with_a_follow_up_keeps_its_line_when_many_campaigns_moved() {
        use crate::monitoring::{
            detection::CampaignMovement,
            metrics::{Window, WindowMetrics},
        };
        let metrics = |spend: f64| WindowMetrics {
            window: Window::SevenDays,
            end_day: CHANGE,
            current: Metrics {
                spend,
                ..Default::default()
            },
            previous: Metrics::default(),
        };
        let (changes, days) = raised();
        let mut detection = detected(&changes, &days, CHANGE + 15);
        let followed = detection.campaigns[0].clone();
        detection.campaigns = (1..=6)
            .map(|id| CampaignMovement {
                campaign_id: 800 + id,
                name: format!("Search - Other {id}"),
                metrics: metrics(1000.0 * id as f64),
                movements: vec![],
                reasons: vec![],
                account_changes: vec![],
                follow_ups: vec![],
            })
            .chain([CampaignMovement {
                metrics: metrics(1.0),
                ..followed
            }])
            .collect();
        let update = crate::monitoring::update::fallback(&detection);
        assert!(update.lines.iter().any(|line| line.campaign_id == CAMPAIGN));
        assert_eq!(
            update.lines.len(),
            crate::monitoring::update::CAMPAIGN_LIMIT
        );
    }
}
