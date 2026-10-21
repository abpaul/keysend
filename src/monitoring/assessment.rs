//! The Assessment of an Ad Account: how its last 30 days compare with the 30 days before,
//! what worked, what needs attention, and what to watch next. Code sets the Result and
//! writes the evidence; AI only words it from that evidence.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{
    audit::{AuditRow, Counts, Data},
    deep_check::Finding,
    follow_up::COUNT_NOISE_DEVIATIONS,
    metrics::{english_change, english_count, english_money, english_number, ratio},
    tracking,
};

/// From this many conversions in the 30 days, a Recommendation keeps the confidence of its rule.
pub const SURE_CONVERSIONS: f64 = 30.0;
/// Fewer conversions than this in the two periods together cannot tell a change from chance.
pub const MIN_RESULTS: f64 = 20.0;
/// A cost per conversion or a return on ad spend counts as changed from this share on.
pub const EFFICIENCY_SHARE: f64 = 0.10;
/// A fall is seasonal when the same period last year fell by at least this share of it.
pub const SEASONAL_SHARE: f64 = 0.5;
pub const LOCATIONS_LISTED: usize = 5;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Improved,
    Steady,
    Mixed,
    Worse,
    Seasonal,
    Unclear,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Improved => "improved",
            Self::Steady => "steady",
            Self::Mixed => "mixed",
            Self::Worse => "worse",
            Self::Seasonal => "seasonal",
            Self::Unclear => "unclear",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Judged {
    pub verdict: Verdict,
    pub reasons: Vec<String>,
}

fn beyond_noise(current: f64, previous: f64) -> bool {
    (current - previous).abs() > COUNT_NOISE_DEVIATIONS * (current + previous).sqrt()
}

/// An Ad Account is judged by value when its conversion value is at least its spend, as
/// for sales; a lead value of one per conversion leaves it judged by cost per conversion.
pub fn by_value(current: &Counts) -> bool {
    current.value > 0.0 && current.value >= current.cost
}

fn count(value: f64) -> String {
    english_count(value)
}

fn rate(value: f64) -> String {
    english_number(value, 2)
}

pub fn judge(data: &Data, judged: bool, findings: &[Finding]) -> Judged {
    let unclear = |reason: &str| Judged {
        verdict: Verdict::Unclear,
        reasons: vec![reason.into()],
    };
    if findings
        .iter()
        .any(|finding| finding.rule.key.starts_with("tracking"))
    {
        return unclear(
            "Conversion tracking needs to be checked first, so results cannot be judged.",
        );
    }
    if !judged {
        return unclear("Results cannot be judged.");
    }
    let (recorded, Some(before)) = (&data.reporting, data.audit.previous.as_ref()) else {
        return unclear("There are no earlier 30 days to compare with.");
    };
    let money = |value: f64| english_money(value, &data.currency);
    let lag = tracking::lag(&data.audit.lag).filter(|lag| lag.missing.0 >= tracking::LAG_SHARE);
    let complete = |counts: &Counts, missing: f64| Counts {
        conversions: counts.conversions / (1.0 - missing),
        value: counts.value / (1.0 - missing),
        ..counts.clone()
    };
    let (now, before) = match &lag {
        Some(lag) => (
            &complete(recorded, lag.missing.0),
            &complete(before, lag.missing.1),
        ),
        None => (recorded, before),
    };
    if now.conversions + before.conversions < MIN_RESULTS {
        return unclear(&format!(
            "With {} conversions against {} in the 30 days before, a change cannot be told apart from chance.",
            count(now.conversions),
            count(before.conversions)
        ));
    }
    let mut reasons = vec![format!(
        "Conversions: {} against {} in the 30 days before ({}).",
        count(recorded.conversions),
        count(
            data.audit
                .previous
                .as_ref()
                .map_or(0.0, |before| before.conversions)
        ),
        english_change(
            recorded.conversions,
            data.audit
                .previous
                .as_ref()
                .map_or(0.0, |before| before.conversions)
        )
    )];
    if let Some(lag) = &lag {
        reasons.push(format!(
            "About {}% of the conversions of the last 30 days have probably not been recorded yet, because {}% of conversions arrive more than a week after the click. With them, conversions would be about {} against {}, and the comparisons below count them.",
            english_number(lag.missing.0 * 100.0, 0),
            english_number(lag.late * 100.0, 0),
            english_number(now.conversions, 0),
            english_number(before.conversions, 0)
        ));
    }
    let significant = beyond_noise(now.conversions, before.conversions);
    if !significant {
        reasons.push(format!(
            "This change in conversions is within normal variation: chance alone moves counts like these by up to {} conversions.",
            english_number(COUNT_NOISE_DEVIATIONS * (now.conversions + before.conversions).sqrt(), 0)
        ));
    }
    let more = significant && now.conversions > before.conversions;
    let fewer = significant && now.conversions < before.conversions;
    let (better, worse) = if now.judged_by_value() {
        let (current, previous) = (ratio(now.value, now.cost), ratio(before.value, before.cost));
        if let Some((current, previous)) = current.zip(previous) {
            reasons.push(format!(
                "Conversion value: {} against {}. Return on ad spend: {} against {}.",
                money(now.value),
                money(before.value),
                rate(current),
                rate(previous)
            ));
        }
        let change = current.zip(previous).and_then(|(c, p)| ratio(c, p));
        (
            change.is_some_and(|change| change >= 1.0 + EFFICIENCY_SHARE),
            change.is_some_and(|change| change <= 1.0 - EFFICIENCY_SHARE),
        )
    } else {
        let (current, previous) = (
            ratio(now.cost, now.conversions),
            ratio(before.cost, before.conversions),
        );
        if let Some((current, previous)) = current.zip(previous) {
            reasons.push(format!(
                "Cost per conversion{}: {} against {} ({}).",
                if lag.is_some() {
                    ", counting the conversions still to come, about"
                } else {
                    ""
                },
                money(current),
                money(previous),
                english_change(current, previous)
            ));
        }
        let change = current.zip(previous).and_then(|(c, p)| ratio(c, p));
        if change.is_some_and(|change| (change - 1.0).abs() >= EFFICIENCY_SHARE) {
            reasons.push(split(now, before, &data.currency));
        }
        (
            change.is_some_and(|change| change <= 1.0 - EFFICIENCY_SHARE),
            change.is_some_and(|change| change >= 1.0 + EFFICIENCY_SHARE),
        )
    };
    let verdict = match (more, fewer, better, worse) {
        (true, _, _, true) | (_, true, true, _) => Verdict::Mixed,
        (true, _, _, _) | (false, false, true, _) => Verdict::Improved,
        (_, true, _, false) if seasonal(data, &mut reasons) => Verdict::Seasonal,
        (_, true, _, _) | (false, false, _, true) => Verdict::Worse,
        _ => Verdict::Steady,
    };
    Judged { verdict, reasons }
}

/// A cost per conversion is the cost per click divided by the conversion rate, so these two
/// show which side of it moved.
fn split(now: &Counts, before: &Counts, currency: &str) -> String {
    let click = |counts: &Counts| ratio(counts.cost, counts.clicks).unwrap_or_default();
    let rate =
        |counts: &Counts| ratio(counts.conversions * 100.0, counts.clicks).unwrap_or_default();
    format!(
        "Cost per click: {} against {} ({}). Conversion rate: {}% against {}% ({}).",
        english_money(click(now), currency),
        english_money(click(before), currency),
        english_change(click(now), click(before)),
        english_number(rate(now), 2),
        english_number(rate(before), 2),
        english_change(rate(now), rate(before))
    )
}

fn seasonal(data: &Data, reasons: &mut Vec<String>) -> bool {
    let (Some([same, before]), Some(previous)) =
        (data.audit.last_year.as_ref(), data.audit.previous.as_ref())
    else {
        return false;
    };
    let (Some(last_year), Some(this_year)) = (
        ratio(same.conversions, before.conversions),
        ratio(data.reporting.conversions, previous.conversions),
    ) else {
        return false;
    };
    let matches = last_year < 1.0 && 1.0 - last_year >= SEASONAL_SHARE * (1.0 - this_year);
    if matches {
        reasons.push(format!(
            "In the same 30 days last year, conversions were {} against {} in the 30 days before them ({}).",
            count(same.conversions),
            count(before.conversions),
            english_change(same.conversions, before.conversions)
        ));
    }
    matches
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Assessment {
    pub verdict: Verdict,
    pub headline: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub worked: Vec<super::claims::Point>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attention: Vec<super::claims::Point>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub market: Option<String>,
    #[serde(default)]
    pub written_without_ai: bool,
}

pub const HEADLINE_LENGTH: usize = 300;
pub const POINT_LENGTH: usize = 280;
pub const NEXT_LENGTH: usize = 200;
pub const MARKET_LENGTH: usize = 300;
pub const POINTS: usize = 3;

impl Verdict {
    fn sentence(self) -> &'static str {
        match self {
            Self::Improved => "Results improved.",
            Self::Steady => "Results held steady.",
            Self::Mixed => "Results were mixed.",
            Self::Worse => "Results worsened.",
            Self::Seasonal => "Results fell in line with the season.",
            Self::Unclear => "",
        }
    }
}

/// The Assessment that code writes when there is no AI answer: the Result and its evidence.
pub fn fallback(judged: &Judged) -> Assessment {
    let season = judged
        .reasons
        .last()
        .filter(|_| judged.verdict == Verdict::Seasonal && judged.reasons.len() > 2);
    let headline = std::iter::once(judged.verdict.sentence())
        .filter(|sentence| !sentence.is_empty())
        .chain(judged.reasons.iter().take(2).map(String::as_str))
        .chain(season.map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ");
    Assessment {
        verdict: judged.verdict,
        headline,
        worked: Vec::new(),
        attention: Vec::new(),
        next: Vec::new(),
        market: None,
        written_without_ai: true,
    }
}

pub(crate) fn percent(part: f64, whole: f64) -> Option<String> {
    ratio(part, whole).map(|share| format!("{}%", english_number(share * 100.0, 0)))
}

pub(crate) fn block(counts: &Counts, currency: &str, judged: bool, by_value: bool) -> Value {
    let money = |value: f64| english_money(value, currency);
    let mut facts = json!({
        "Spend": money(counts.cost),
        "Clicks": count(counts.clicks),
        "Impressions": count(counts.impressions),
        "CTR": ratio(counts.clicks * 100.0, counts.impressions).map(|rate| format!("{}%", english_number(rate, 2))),
        "CPC": ratio(counts.cost, counts.clicks).map(money),
    });
    if judged {
        facts["Conversions"] = count(counts.conversions).into();
        facts["Conversion rate"] = ratio(counts.conversions * 100.0, counts.clicks)
            .map(|rate| format!("{}%", english_number(rate, 2)))
            .into();
        facts["Cost per conversion"] = ratio(counts.cost, counts.conversions).map(money).into();
    }
    if judged && by_value {
        facts["Conversion value"] = money(counts.value).into();
        facts["Return on ad spend"] = ratio(counts.value, counts.cost).map(rate).into();
        facts["Value per conversion"] = ratio(counts.value, counts.conversions).map(money).into();
    }
    facts
}

pub(crate) fn changes(now: &Counts, before: &Counts, judged: bool, by_value: bool) -> Value {
    let change = |current: Option<f64>, previous: Option<f64>| {
        current
            .zip(previous)
            .map(|(current, previous)| english_change(current, previous))
    };
    let mut facts = json!({
        "Spend": english_change(now.cost, before.cost),
        "Clicks": english_change(now.clicks, before.clicks),
        "CPC": change(ratio(now.cost, now.clicks), ratio(before.cost, before.clicks)),
    });
    if judged {
        let mut conversions = english_change(now.conversions, before.conversions);
        if !beyond_noise(now.conversions, before.conversions) {
            conversions.push_str(&format!(
                " (within normal variation: chance alone moves counts like these by up to {} conversions)",
                english_number(COUNT_NOISE_DEVIATIONS * (now.conversions + before.conversions).sqrt(), 0)
            ));
        }
        facts["Conversions"] = conversions.into();
        facts["Cost per conversion"] = small(
            change(
                ratio(now.cost, now.conversions),
                ratio(before.cost, before.conversions),
            ),
            ratio(now.cost, now.conversions).zip(ratio(before.cost, before.conversions)),
        )
        .into();
    }
    if judged && by_value {
        facts["Conversion value"] = english_change(now.value, before.value).into();
        facts["Return on ad spend"] = small(
            change(ratio(now.value, now.cost), ratio(before.value, before.cost)),
            ratio(now.value, now.cost).zip(ratio(before.value, before.cost)),
        )
        .into();
    }
    facts
}

/// Marks a change of a cost or a return below the share that counts as a change.
fn small(change: Option<String>, values: Option<(f64, f64)>) -> Option<String> {
    let (current, previous) = values?;
    let mut change = change?;
    if (current / previous - 1.0).abs() < EFFICIENCY_SHARE {
        change.push_str(" (a small change)");
    }
    Some(change)
}

fn target(cost_per: Option<f64>, target: Option<f64>, currency: &str) -> Value {
    let Some(target) = target else {
        return Value::Null;
    };
    let money = |value: f64| english_money(value, currency);
    match cost_per {
        Some(cost) if cost > target => {
            json!({"target": money(target), "against_target": format!("{} above the target of {}", english_change(cost, target).replace("up ", ""), money(target))})
        }
        Some(_) => {
            json!({"target": money(target), "against_target": format!("at or below the target of {}", money(target))})
        }
        None => json!({"target": money(target)}),
    }
}

/// The evidence for the AI: every number is computed and formatted here.
pub struct Evidence<'a> {
    pub data: &'a Data,
    pub judged: bool,
    pub result: &'a Judged,
    pub campaigns: &'a [crate::workspace::history::Campaign],
    pub target: &'a dyn Fn(Option<i64>) -> Option<f64>,
}

pub fn facts(evidence: &Evidence) -> Value {
    let data = evidence.data;
    let (now, judged) = (&data.reporting, evidence.judged);
    let by_value = now.judged_by_value();
    let currency = data.currency.as_str();
    let mut account = json!({
        "last_30_days": block(now, currency, judged, by_value),
        "previous_30_days": data.audit.previous.as_ref().map(|before| block(before, currency, judged, by_value)),
        "changes": data.audit.previous.as_ref().map(|before| changes(now, before, judged, by_value)),
        "judged_by": if !judged { "nothing: results cannot be judged" } else if by_value { "conversion value and return on ad spend" } else { "conversions and cost per conversion" },
    });
    if judged {
        account["target"] = target(
            ratio(now.cost, now.conversions),
            (evidence.target)(None),
            currency,
        );
    }
    if !data.audit.economics.is_empty() {
        account["economics"] = data.audit.economics.facts(currency);
    }
    if let Some([same, before]) = &data.audit.last_year {
        account["same_30_days_last_year"] = block(same, currency, judged, by_value);
        account["30_days_before_them_last_year"] = block(before, currency, judged, by_value);
    }
    let campaigns = data
        .audit
        .campaigns
        .iter()
        .map(|row| campaign(row, evidence, by_value))
        .collect::<Vec<_>>();
    let brand = |row: &&AuditRow| {
        row.name
            .split(|character: char| !character.is_alphanumeric())
            .any(|word| word.eq_ignore_ascii_case("brand"))
    };
    if data.audit.campaigns.iter().any(|row| brand(&row))
        && data.audit.campaigns.iter().any(|row| !brand(&row))
    {
        let mut other = Counts::default();
        for row in data.audit.campaigns.iter().filter(|row| !brand(row)) {
            other.impressions += row.counts.impressions;
            other.clicks += row.counts.clicks;
            other.cost += row.counts.cost;
            other.conversions += row.counts.conversions;
            other.value += row.counts.value;
        }
        account["without_brand_campaigns"] = block(&other, currency, judged, by_value);
    }
    let mut locations = data.audit.locations.iter().collect::<Vec<_>>();
    locations.sort_by(|a, b| b.counts.cost.total_cmp(&a.counts.cost));
    let locations = locations
        .into_iter()
        .take(LOCATIONS_LISTED)
        .map(|row| {
            let mut facts = json!({
                "name": row.name,
                "Spend": english_money(row.counts.cost, currency),
                "share_of_spend": percent(row.counts.cost, now.cost),
            });
            if judged {
                facts["Conversions"] = count(row.counts.conversions).into();
                facts["share_of_conversions"] =
                    percent(row.counts.conversions, now.conversions).into();
                facts["Cost per conversion"] = ratio(row.counts.cost, row.counts.conversions)
                    .map(|cost| english_money(cost, currency))
                    .into();
            }
            facts
        })
        .collect::<Vec<_>>();
    let lag = tracking::lag(&data.audit.lag).map(|lag| json!({
        "share_of_conversions_arriving_after_a_week": format!("{}%", english_number(lag.late * 100.0, 0)),
        "share_of_last_30_days_not_recorded_yet": format!("{}%", english_number(lag.missing.0 * 100.0, 0)),
    }));
    json!({
        "period": "the 30 days before the Deep check, against the 30 days before them",
        "conversion_lag": lag,
        "result": {"verdict": evidence.result.verdict, "evidence": evidence.result.reasons},
        "account": account,
        "campaigns": campaigns,
        "locations": locations,
        "judge_by_conversions": judged,
    })
}

fn campaign(row: &AuditRow, evidence: &Evidence, by_value: bool) -> Value {
    let data = evidence.data;
    let (now, judged, currency) = (&data.reporting, evidence.judged, data.currency.as_str());
    let known = row
        .id
        .and_then(|id| evidence.campaigns.iter().find(|campaign| campaign.id == id));
    let mut facts = json!({
        "name": row.name,
        "type": known.map(|campaign| campaign.channel_type.to_lowercase().replace('_', " ")),
        "daily_budget": known.and_then(|campaign| campaign.budget_micros).map(|micros| english_money(micros as f64 / 1_000_000.0, currency)),
        "last_30_days": block(&row.counts, currency, judged, by_value),
        "previous_30_days": row.previous.as_ref().map(|before| block(before, currency, judged, by_value)),
        "changes": row.previous.as_ref().map(|before| changes(&row.counts, before, judged, by_value)),
        "share_of_spend": percent(row.counts.cost, now.cost),
        "search_impressions_lost_to_budget": row.lost_to_budget.filter(|lost| *lost > 0.0).map(|lost| format!("{}%", english_number(lost * 100.0, 0))),
    });
    if judged {
        facts["share_of_conversions"] = percent(row.counts.conversions, now.conversions).into();
        facts["target"] = target(
            ratio(row.counts.cost, row.counts.conversions),
            (evidence.target)(row.id),
            currency,
        );
    }
    if judged && by_value {
        facts["share_of_value"] = percent(row.counts.value, now.value).into();
    }
    facts
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct Answer {
    #[serde(default)]
    pub headline: String,
    #[serde(default)]
    pub worked: Vec<super::claims::Answer>,
    #[serde(default)]
    pub attention: Vec<super::claims::Answer>,
    #[serde(default)]
    pub next: Vec<String>,
    #[serde(default)]
    pub market: Option<String>,
}

pub(crate) fn schema(evidence: &[String]) -> Value {
    let point = json!({"type":"object","additionalProperties":false,"required":["observation","explanation","evidence"],"properties":{
        "observation":{"type":"string","maxLength":POINT_LENGTH},
        "explanation":{"type":["string","null"],"maxLength":POINT_LENGTH},
        "evidence":{"type":"array","minItems":1,"maxItems":3,"items":{"type":"string","enum":evidence}},
    }});
    json!({
        "headline": {"type":"string","maxLength":HEADLINE_LENGTH},
        "worked": {"type":"array","maxItems":POINTS,"items":point},
        "attention": {"type":"array","maxItems":POINTS,"items":point},
        "next": {"type":"array","minItems":1,"maxItems":2,"items":{"type":"string","maxLength":NEXT_LENGTH}},
        "market": {"type":["string","null"],"maxLength":MARKET_LENGTH},
    })
}

/// Keeps each text of the answer that rests on the evidence. A point of what worked or what
/// needs attention must rest on the evidence that it cites; the headline and the priorities on
/// any of it. Any text may give a reason only as a possibility.
pub(crate) fn accept(
    answer: Answer,
    result: &Judged,
    facts: &Value,
    names: &[&str],
    market: &Value,
    rules_out: impl Fn(&str) -> bool,
) -> Assessment {
    let items = super::claims::evidence(facts);
    let anywhere = |text: &str, length: usize| {
        super::claims::grounded(text, facts, length, names, false)
            .or_else(|_| {
                crate::ai::valid_hedged(
                    text,
                    &[&crate::ai::Known::of(&[facts])],
                    length,
                    names,
                    false,
                )
                .and_then(|()| {
                    if super::update::numeric_tokens(text)
                        .is_subset(&super::update::numeric_tokens(&facts.to_string()))
                    {
                        Ok(())
                    } else {
                        Err("number differs from its displayed string")
                    }
                })
            })
            .and_then(|()| {
                if rules_out(text) {
                    Err("results cannot be judged")
                } else {
                    Ok(())
                }
            })
            .map_err(|reason| tracing::warn!(reason, "Assessment text rejected"))
            .is_ok()
    };
    let points = |answers: Vec<super::claims::Answer>| {
        answers
            .into_iter()
            .take(POINTS)
            .filter_map(|answer| {
                super::claims::point(answer, &items, POINT_LENGTH, names)
                    .and_then(|point| {
                        let text = format!(
                            "{} {}",
                            point.observation,
                            point.explanation.as_deref().unwrap_or_default()
                        );
                        if rules_out(&text) {
                            Err("results cannot be judged")
                        } else {
                            Ok(point)
                        }
                    })
                    .map_err(|reason| tracing::warn!(reason, "Assessment point rejected"))
                    .ok()
            })
            .collect::<Vec<_>>()
    };
    if !anywhere(&answer.headline, HEADLINE_LENGTH) {
        return fallback(result);
    }
    Assessment {
        verdict: result.verdict,
        headline: answer.headline,
        worked: points(answer.worked),
        attention: points(answer.attention),
        next: answer
            .next
            .into_iter()
            .take(POINTS)
            .filter(|text| anywhere(text, NEXT_LENGTH))
            .collect(),
        market: answer.market.filter(|text| {
            !market.is_null()
                && super::claims::grounded(text, market, MARKET_LENGTH, names, false)
                    .map_err(|reason| tracing::warn!(reason, "Market context rejected"))
                    .is_ok()
        }),
        written_without_ai: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitoring::audit::Audit;

    fn counts(cost: f64, clicks: f64, conversions: f64, value: f64) -> Counts {
        Counts {
            impressions: clicks * 20.0,
            clicks,
            conversions,
            cost,
            value,
        }
    }

    fn data(now: Counts, before: Counts, last_year: Option<[Counts; 2]>) -> Data {
        Data {
            currency: "USD".into(),
            reporting: now,
            audit: Audit {
                previous: Some(before),
                last_year,
                ..Default::default()
            },
        }
    }

    fn judged(data: &Data) -> Judged {
        judge(data, true, &[])
    }

    #[test]
    fn the_result_weighs_conversions_beyond_chance_against_the_cost_of_each() {
        let result = judged(&data(
            counts(4200.0, 2000.0, 168.0, 0.0),
            counts(3000.0, 1800.0, 252.0, 0.0),
            None,
        ));
        assert_eq!(result.verdict, Verdict::Worse);
        assert_eq!(
            result.reasons,
            [
                "Conversions: 168 against 252 in the 30 days before (down 33%).",
                "Cost per conversion: $25.00 against $11.90 (up 110%).",
                "Cost per click: $2.10 against $1.67 (up 26%). Conversion rate: 8.40% against 14.00% (down 40%)."
            ]
        );
        assert_eq!(
            judged(&data(
                counts(4200.0, 2000.0, 300.0, 0.0),
                counts(3000.0, 1800.0, 252.0, 0.0),
                None
            ))
            .verdict,
            Verdict::Mixed
        );
        assert_eq!(
            judged(&data(
                counts(2600.0, 2000.0, 260.0, 0.0),
                counts(3000.0, 1800.0, 252.0, 0.0),
                None
            ))
            .verdict,
            Verdict::Improved
        );
        assert_eq!(
            judged(&data(
                counts(3050.0, 2000.0, 255.0, 0.0),
                counts(3000.0, 1800.0, 252.0, 0.0),
                None
            ))
            .verdict,
            Verdict::Steady
        );
    }

    #[test]
    fn a_fall_that_the_same_days_of_last_year_also_had_is_seasonal() {
        let fall = (
            counts(2850.0, 1350.0, 96.0, 0.0),
            counts(4200.0, 2100.0, 150.0, 0.0),
        );
        let result = judged(&data(
            fall.0.clone(),
            fall.1.clone(),
            Some([
                counts(2850.0, 1320.0, 93.0, 0.0),
                counts(4200.0, 2100.0, 150.0, 0.0),
            ]),
        ));
        assert_eq!(result.verdict, Verdict::Seasonal);
        assert!(fallback(&result)
            .headline
            .ends_with("In the same 30 days last year, conversions were 93 against 150 in the 30 days before them (down 38%)."));
        assert_eq!(
            result.reasons.last().unwrap(),
            "In the same 30 days last year, conversions were 93 against 150 in the 30 days before them (down 38%)."
        );
        assert_eq!(judged(&data(fall.0, fall.1, None)).verdict, Verdict::Worse);
    }

    #[test]
    fn few_conversions_untracked_results_or_a_tracking_finding_leave_the_result_unclear() {
        let few = data(
            counts(390.0, 195.0, 5.0, 0.0),
            counts(360.0, 180.0, 8.0, 0.0),
            None,
        );
        assert_eq!(
            judged(&few).reasons,
            ["With 5 conversions against 8 in the 30 days before, a change cannot be told apart from chance."]
        );
        let tracked = data(
            counts(4200.0, 2000.0, 168.0, 0.0),
            counts(3000.0, 1800.0, 252.0, 0.0),
            None,
        );
        assert_eq!(judge(&tracked, false, &[]).verdict, Verdict::Unclear);
        let stopped = Finding {
            campaign_id: None,
            rule: crate::monitoring::findings::Finding {
                key: "tracking-stopped".into(),
                title: String::new(),
                detail: String::new(),
                subjects: vec![],
                spend: 0.0,
            },
        };
        assert_eq!(judge(&tracked, true, &[stopped]).verdict, Verdict::Unclear);
    }

    #[test]
    fn a_sales_account_is_judged_by_its_return_on_ad_spend() {
        let result = judged(&data(
            counts(12600.0, 17700.0, 408.0, 110400.0),
            counts(10200.0, 15600.0, 360.0, 114000.0),
            None,
        ));
        assert_eq!(result.verdict, Verdict::Worse);
        assert_eq!(
            result.reasons[2],
            "Conversion value: $110,400 against $114,000. Return on ad spend: 8.76 against 11.18."
        );
    }

    #[test]
    fn without_ai_the_headline_is_the_result_and_its_evidence() {
        let result = judged(&data(
            counts(4200.0, 2000.0, 168.0, 0.0),
            counts(3000.0, 1800.0, 252.0, 0.0),
            None,
        ));
        assert_eq!(fallback(&result).headline, "Results worsened. Conversions: 168 against 252 in the 30 days before (down 33%). Cost per conversion: $25.00 against $11.90 (up 110%).");
    }

    #[test]
    fn conversions_that_have_not_arrived_yet_do_not_make_a_steady_account_look_worse() {
        let mut lagging = data(
            counts(8400.0, 2250.0, 98.0, 0.0),
            counts(8400.0, 2250.0, 108.0, 0.0),
            None,
        );
        assert_eq!(judged(&lagging).verdict, Verdict::Worse);
        lagging.audit.lag = [
            ("LESS_THAN_ONE_DAY", 60.0),
            ("SEVEN_TO_EIGHT_DAYS", 20.0),
            ("FOURTEEN_TO_TWENTY_ONE_DAYS", 20.0),
        ]
        .map(|(bucket, conversions)| (bucket.to_owned(), conversions))
        .to_vec();
        let result = judged(&lagging);
        assert_eq!(result.verdict, Verdict::Steady);
        assert_eq!(result.reasons[1], "About 18% of the conversions of the last 30 days have probably not been recorded yet, because 40% of conversions arrive more than a week after the click. With them, conversions would be about 120 against 108, and the comparisons below count them.");
    }
}
