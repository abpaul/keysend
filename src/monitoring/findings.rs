use serde::{Deserialize, Serialize};

use super::{
    audit::{Audit, AuditRow, Counts, Data},
    follow_up::COUNT_NOISE_DEVIATIONS,
    metrics::{english_change, english_count, english_money, english_number},
    tracking::MIN_CONVERSIONS,
};
use crate::{language::english, workspace::history::date_of};

const UNCERTAIN_CONVERSIONS: f64 = 30.0;
const CLICKS: f64 = 30.0;
const PAGE_CLICKS: f64 = 100.0;
const FAR: f64 = 2.0;
const SHARE: f64 = 0.02;
const LISTED: usize = 5;
const PROVEN_CONVERSIONS: f64 = 5.0;
const VERY_FAR: f64 = 4.0;
/// Clicks that rose by this share without more conversions are traffic without results.
const TRAFFIC_RISE: f64 = 0.2;
/// A cost per click that rose by this share is a rising click cost.
const CLICK_COST_RISE: f64 = 0.2;
/// A cost per conversion that rose by this share makes either rise a Finding.
const COST_RISE: f64 = 0.1;
const PERIOD_CLICKS: f64 = 100.0;
/// A location is costly only when it has at least this share of the spend.
const LOCATION_SHARE: f64 = 0.25;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Finding {
    pub key: String,
    pub title: String,
    pub detail: String,
    pub subjects: Vec<String>,
    pub spend: f64,
}

fn tracking(data: &Data) -> &'static str {
    if data.audit.conversion_actions == Some(0) {
        "not set up"
    } else if data.audit.stopped.is_some() && data.reporting.conversions > 0.0 {
        "stopped"
    } else if data.audit.spike.is_some() {
        "spike"
    } else if data.reporting.conversions <= 0.0 {
        "no results"
    } else if data.reporting.conversions < MIN_CONVERSIONS {
        "few results"
    } else if data.reporting.conversions < UNCERTAIN_CONVERSIONS {
        "uncertain"
    } else {
        "tracked"
    }
}

struct Money<'a>(&'a str);

impl Money<'_> {
    fn of(&self, value: f64) -> String {
        english_money(value, self.0)
    }
}

fn names(rows: &[&AuditRow]) -> String {
    let mut text = rows
        .iter()
        .take(LISTED)
        .map(|row| row.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    if rows.len() > LISTED {
        text.push_str(&format!(", and {} more", rows.len() - LISTED));
    }
    text
}

fn costly(counts: &Counts, account: f64) -> bool {
    counts.clicks >= CLICKS
        && ((counts.conversions >= PROVEN_CONVERSIONS
            && ratio(counts.cost, counts.conversions).is_some_and(|value| value >= account * FAR))
            || (counts.conversions <= 1.0 && counts.cost >= account * VERY_FAR))
}

fn counted(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

fn scope(campaign: Option<&str>) -> String {
    match campaign {
        Some(campaign) => format!("In the 30 days before the Deep check, {campaign} had"),
        None => "In the 30 days before the Deep check, the Campaign had".into(),
    }
}

fn count(value: f64) -> String {
    english_number(value, if value.fract() == 0.0 { 0 } else { 2 })
}

fn ratio(numerator: f64, denominator: f64) -> Option<f64> {
    (denominator > 0.0).then(|| numerator / denominator)
}

fn percent(part: f64, whole: f64) -> String {
    format!(
        "{}%",
        english_number(ratio(part, whole).unwrap_or_default() * 100.0, 0)
    )
}

fn beyond_noise(current: f64, previous: f64) -> bool {
    (current - previous).abs() > COUNT_NOISE_DEVIATIONS * (current + previous).sqrt()
}

fn more(current: f64, previous: f64) -> bool {
    current > previous && beyond_noise(current, previous)
}

fn rose(current: Option<f64>, previous: Option<f64>, share: f64) -> bool {
    current
        .zip(previous)
        .is_some_and(|(current, previous)| current >= previous * (1.0 + share))
}

fn date(day: i64) -> String {
    date_of(day).map_or_else(String::new, |date| english().written.format.day_month(date))
}

/// Rules that compare a Campaign with its own 30 days before.
fn changed(row: &AuditRow, money: &Money, material: impl Fn(f64) -> bool) -> Option<Finding> {
    let (now, before) = (&row.counts, row.previous.as_ref()?);
    let cost_now = ratio(now.cost, now.conversions);
    let cost_before = ratio(before.cost, before.conversions);
    if !material(now.cost)
        || before.clicks < PERIOD_CLICKS
        || more(now.conversions, before.conversions)
        || !rose(cost_now, cost_before, COST_RISE)
    {
        return None;
    }
    let results = format!(
        "{} conversions against {}, and its cost per conversion rose from {} to {}",
        count(now.conversions),
        count(before.conversions),
        money.of(cost_before.unwrap_or_default()),
        money.of(cost_now.unwrap_or_default())
    );
    if now.clicks >= before.clicks * (1.0 + TRAFFIC_RISE) && more(now.clicks, before.clicks) {
        return Some(Finding {
            key: format!("traffic-without-results:{}", row.name),
            title: format!("{} got more clicks but not more conversions", row.name),
            detail: format!(
                "In the 30 days before the Deep check, {} had {} clicks against {} in the previous 30 days ({}), but {}.",
                row.name,
                english_count(now.clicks),
                english_count(before.clicks),
                english_change(now.clicks, before.clicks),
                results
            ),
            subjects: vec![row.name.clone()],
            spend: now.cost,
        });
    }
    let click_now = ratio(now.cost, now.clicks);
    let click_before = ratio(before.cost, before.clicks);
    (now.clicks >= PERIOD_CLICKS && rose(click_now, click_before, CLICK_COST_RISE)).then(|| {
        Finding {
            key: format!("click-cost-rising:{}", row.name),
            title: format!("{} pays more for each click", row.name),
            detail: format!(
                "In the 30 days before the Deep check, {} paid {} per click against {} in the previous 30 days ({}), with {}.",
                row.name,
                money.of(click_now.unwrap_or_default()),
                money.of(click_before.unwrap_or_default()),
                english_change(click_now.unwrap_or_default(), click_before.unwrap_or_default()),
                results
            ),
            subjects: vec![row.name.clone()],
            spend: now.cost,
        }
    })
}

pub fn detect(
    data: &Data,
    judge_by_conversions: bool,
    account_label: &str,
    campaign: Option<&str>,
) -> Vec<Finding> {
    let money = Money(&data.currency);
    let total = &data.reporting;
    let audit: &Audit = &data.audit;
    let mut findings = Vec::new();
    let material = |spend: f64| spend >= total.cost * SHARE && spend >= 1.0;
    let subjects = |rows: &[&AuditRow]| {
        rows.iter()
            .take(LISTED)
            .map(|row| row.name.clone())
            .collect::<Vec<_>>()
    };
    match tracking(data) {
        "stopped" => {
            if let Some(stopped) = &audit.stopped {
                findings.push(Finding {
                    key: "tracking-stopped".into(),
                    title: "Conversions stopped while clicks continued".into(),
                    detail: format!(
                        "No conversion has been recorded since {} ({} complete days), while the Ad Account had {} clicks and {} spend in those days. In the 30 days before {}, it recorded {} conversions.",
                        date(stopped.since),
                        stopped.days,
                        english_count(stopped.clicks),
                        money.of(stopped.cost),
                        date(stopped.since),
                        count(stopped.before)
                    ),
                    subjects: vec![],
                    spend: stopped.cost,
                });
            }
        }
        "spike" => {
            if let Some(spike) = &audit.spike {
                findings.push(Finding {
                    key: "tracking-spike".into(),
                    title: "Conversions jumped while clicks did not".into(),
                    detail: format!(
                        "Since {} ({} complete days), the Ad Account recorded {} conversions per 100 clicks, against {} in the 28 days before, while its clicks per day held.",
                        date(spike.since),
                        spike.days,
                        english_number(spike.rate * 100.0, 1),
                        english_number(spike.usual * 100.0, 1)
                    ),
                    subjects: vec![],
                    spend: total.cost,
                });
            }
        }
        "not set up" => findings.push(Finding {
            key: "tracking-not-set-up".into(),
            title: "Conversion tracking is not set up".into(),
            detail: format!(
                "The Ad Account has no active conversion action. {} spend and {} clicks cannot be judged by results.",
                money.of(total.cost),
                count(total.clicks)
            ),
            subjects: vec![],
            spend: total.cost,
        }),
        "no results" if total.clicks >= PAGE_CLICKS => findings.push(Finding {
            key: "tracking-no-results".into(),
            title: "No conversions were recorded".into(),
            detail: format!(
                "{} clicks and {} spend produced 0 conversions. Conversion actions exist, so check that they still record.",
                count(total.clicks),
                money.of(total.cost)
            ),
            subjects: vec![],
            spend: total.cost,
        }),
        _ => {}
    }
    let campaigns = audit.campaigns.iter().collect::<Vec<_>>();
    if judge_by_conversions {
        let Some(account) = ratio(total.cost, total.conversions) else {
            return findings;
        };
        let rate = ratio(total.conversions, total.clicks).unwrap_or_default();
        for (key, one, many, rows) in [
            (
                "search-terms-no-conversions",
                "search term",
                "search terms",
                &audit.search_terms,
            ),
            (
                "keywords-no-conversions",
                "keyword",
                "keywords",
                &audit.keywords,
            ),
        ] {
            let mut rows = rows
                .iter()
                .filter(|row| row.counts.conversions <= 0.0 && row.counts.cost >= account)
                .collect::<Vec<_>>();
            rows.sort_by(|a, b| b.counts.cost.total_cmp(&a.counts.cost));
            let spend = rows.iter().map(|row| row.counts.cost).sum::<f64>();
            if spend >= account * FAR && material(spend) {
                findings.push(Finding {
                    key: key.into(),
                    title: format!(
                        "{} had spend and no conversions",
                        counted(rows.len(), one, many)
                    ),
                    detail: format!(
                        "{} {} spend and 0 conversions on {}, each with spend at or above the {account_label} cost per conversion of {}: {}.",
                        scope(campaign),
                        money.of(spend),
                        counted(rows.len(), one, many),
                        money.of(account),
                        names(&rows)
                    ),
                    subjects: subjects(&rows),
                    spend,
                });
            }
        }
        for row in &campaigns {
            let cost_per = ratio(row.counts.cost, row.counts.conversions);
            if let Some(lost) = row.lost_to_budget.filter(|lost| *lost >= 0.1) {
                if row.counts.conversions >= 5.0 && cost_per.is_some_and(|value| value <= account) {
                    findings.push(Finding {
                        key: format!("budget-limited:{}", row.name),
                        title: format!("{} is limited by its budget", row.name),
                        detail: format!(
                            "{} lost {}% of its estimated eligible Search impressions to budget. Its cost per conversion is {}, at or below the {account_label} value of {}, with {} conversions.",
                            row.name,
                            english_number(lost * 100.0, 0),
                            money.of(cost_per.unwrap_or_default()),
                            money.of(account),
                            count(row.counts.conversions)
                        ),
                        subjects: vec![row.name.clone()],
                        spend: row.counts.cost,
                    });
                }
            }
            let returned = ratio(row.counts.value, row.counts.cost);
            let account_return = ratio(total.value, total.cost).unwrap_or_default();
            let low_return = total.judged_by_value()
                && row.counts.clicks >= CLICKS
                && returned.is_some_and(|value| value * FAR <= account_return)
                && material(row.counts.cost);
            let economics = &audit.economics;
            let below_break_even = economics.break_even_return().filter(|threshold| {
                total.judged_by_value()
                    && row.counts.clicks >= CLICKS
                    && returned.is_some_and(|value| value < *threshold)
                    && material(row.counts.cost)
            });
            let above_break_even = economics.break_even_cost().filter(|threshold| {
                !total.judged_by_value()
                    && row.counts.conversions >= PROVEN_CONVERSIONS
                    && cost_per.is_some_and(|cost| cost > *threshold)
                    && material(row.counts.cost)
            });
            if let Some(threshold) = above_break_even {
                findings.push(Finding {
                    key: format!("campaign-above-break-even:{}", row.name),
                    title: format!("{} pays more per conversion than a conversion is worth", row.name),
                    detail: format!(
                        "{} paid {} per conversion for {} conversions. At the value of a conversion in the Ad Account profile, a cost above {} per conversion loses money. It had {} of the spend.",
                        row.name,
                        money.of(cost_per.unwrap_or_default()),
                        count(row.counts.conversions),
                        money.of(threshold),
                        percent(row.counts.cost, total.cost)
                    ),
                    subjects: vec![row.name.clone()],
                    spend: row.counts.cost,
                });
            }
            if !low_return
                && above_break_even.is_none()
                && below_break_even.is_none()
                && costly(&row.counts, account)
                && material(row.counts.cost)
            {
                findings.push(Finding {
                    key: format!("campaign-cost-high:{}", row.name),
                    title: format!("{} has a high cost per conversion", row.name),
                    detail: format!(
                        "{} spent {} for {} conversions ({}). The {account_label} cost per conversion is {}. It had {} of the spend and {} of the conversions.",
                        row.name,
                        money.of(row.counts.cost),
                        count(row.counts.conversions),
                        cost_per.map_or_else(
                            || "no cost per conversion".into(),
                            |value| format!("{} per conversion", money.of(value))
                        ),
                        money.of(account),
                        percent(row.counts.cost, total.cost),
                        percent(row.counts.conversions, total.conversions)
                    ),
                    subjects: vec![row.name.clone()],
                    spend: row.counts.cost,
                });
            }
            if let Some((threshold, margin)) = below_break_even.zip(economics.margin) {
                findings.push(Finding {
                    key: format!("campaign-below-break-even:{}", row.name),
                    title: format!("{} returns less than its margin needs", row.name),
                    detail: format!(
                        "{} spent {} for {} conversion value, a return on ad spend of {}. With a margin of {}%, a return below {} does not cover the ad cost. It had {} of the spend.",
                        row.name,
                        money.of(row.counts.cost),
                        money.of(row.counts.value),
                        english_number(returned.unwrap_or_default(), 2),
                        english_number(margin * 100.0, 0),
                        english_number(threshold, 2),
                        percent(row.counts.cost, total.cost)
                    ),
                    subjects: vec![row.name.clone()],
                    spend: row.counts.cost,
                });
            } else if low_return {
                findings.push(Finding {
                    key: format!("campaign-return-low:{}", row.name),
                    title: format!("{} returns little for its spend", row.name),
                    detail: format!(
                        "{} spent {} for {} conversion value, a return on ad spend of {}. The {account_label} return on ad spend is {}. It had {} of the spend and {} of the conversion value.",
                        row.name,
                        money.of(row.counts.cost),
                        money.of(row.counts.value),
                        english_number(returned.unwrap_or_default(), 2),
                        english_number(account_return, 2),
                        percent(row.counts.cost, total.cost),
                        percent(row.counts.value, total.value)
                    ),
                    subjects: vec![row.name.clone()],
                    spend: row.counts.cost,
                });
            }
            findings.extend(changed(row, &money, material));
        }
        if audit.locations.len() >= 2 {
            for row in &audit.locations {
                let cost_per = ratio(row.counts.cost, row.counts.conversions);
                if costly(&row.counts, account) && row.counts.cost >= total.cost * LOCATION_SHARE {
                    findings.push(Finding {
                        key: format!("location-cost-high:{}", row.name),
                        title: format!("{} costs more per conversion than other locations", row.name),
                        detail: format!(
                            "In the 30 days before the Deep check, {} had {} of the spend and {} of the conversions: {} for {} conversions ({}). The {account_label} cost per conversion is {}.",
                            row.name,
                            percent(row.counts.cost, total.cost),
                            percent(row.counts.conversions, total.conversions),
                            money.of(row.counts.cost),
                            count(row.counts.conversions),
                            cost_per.map_or_else(
                                || "no cost per conversion".into(),
                                |value| format!("{} per conversion", money.of(value))
                            ),
                            money.of(account)
                        ),
                        subjects: vec![row.name.clone()],
                        spend: row.counts.cost,
                    });
                }
            }
        }
        for row in &audit.landing_pages {
            let page_rate = ratio(row.counts.conversions, row.counts.clicks).unwrap_or_default();
            if row.counts.clicks >= PAGE_CLICKS
                && rate > 0.0
                && page_rate <= rate / FAR
                && material(row.counts.cost)
            {
                let better = audit
                    .landing_pages
                    .iter()
                    .filter(|other| other.name != row.name && other.counts.clicks >= CLICKS)
                    .filter_map(|other| {
                        ratio(other.counts.conversions, other.counts.clicks)
                            .filter(|other_rate| *other_rate >= page_rate * FAR)
                            .map(|other_rate| (other, other_rate))
                    })
                    .max_by(|a, b| a.1.total_cmp(&b.1))
                    .map_or_else(String::new, |(other, other_rate)| {
                        format!(
                            " {} converted {}% of its {} clicks.",
                            other.name,
                            english_number(other_rate * 100.0, 2),
                            count(other.counts.clicks)
                        )
                    });
                findings.push(Finding {
                    key: format!("landing-page-low-rate:{}", row.name),
                    title: format!("The landing page {} converts below the {account_label} rate", row.name),
                    detail: format!(
                        "{} had {} clicks and {} conversions, a conversion rate of {}%. The {account_label} conversion rate is {}%.{better}",
                        row.name,
                        count(row.counts.clicks),
                        count(row.counts.conversions),
                        english_number(page_rate * 100.0, 2),
                        english_number(rate * 100.0, 2)
                    ),
                    subjects: vec![row.name.clone()],
                    spend: row.counts.cost,
                });
            }
        }
    } else {
        let click_cost = ratio(total.cost, total.clicks).unwrap_or_default();
        let click_rate = ratio(total.clicks, total.impressions).unwrap_or_default();
        for row in &campaigns {
            let cost = ratio(row.counts.cost, row.counts.clicks).unwrap_or_default();
            let share = ratio(row.counts.clicks, row.counts.impressions).unwrap_or_default();
            if click_cost > 0.0
                && cost >= click_cost * FAR
                && row.counts.clicks >= CLICKS
                && material(row.counts.cost)
            {
                findings.push(Finding {
                    key: format!("campaign-click-cost-high:{}", row.name),
                    title: format!("{} has a high cost per click", row.name),
                    detail: format!(
                        "{} paid {} per click for {} clicks. The {account_label} CPC is {}.",
                        row.name,
                        money.of(cost),
                        count(row.counts.clicks),
                        money.of(click_cost)
                    ),
                    subjects: vec![row.name.clone()],
                    spend: row.counts.cost,
                });
            }
            if let Some(lost) = row.lost_to_budget.filter(|lost| *lost >= 0.1) {
                if share >= click_rate && cost <= click_cost && row.counts.clicks >= CLICKS {
                    findings.push(Finding {
                        key: format!("budget-limited:{}", row.name),
                        title: format!("{} is limited by its budget", row.name),
                        detail: format!(
                            "{} lost {}% of its estimated eligible Search impressions to budget. Its CTR is {}%, at or above the {account_label} CTR of {}%, and its CPC is not above the {account_label} CPC.",
                            row.name,
                            english_number(lost * 100.0, 0),
                            english_number(share * 100.0, 2),
                            english_number(click_rate * 100.0, 2)
                        ),
                        subjects: vec![row.name.clone()],
                        spend: row.counts.cost,
                    });
                }
            }
        }
    }
    let poor = audit
        .ads
        .iter()
        .filter(|row| row.strength == "POOR" && row.counts.impressions > 0.0)
        .collect::<Vec<_>>();
    if !poor.is_empty() {
        let spend = poor.iter().map(|row| row.counts.cost).sum::<f64>();
        findings.push(Finding {
            key: "ads-poor-strength".into(),
            title: format!(
                "{} poor ad strength",
                counted(poor.len(), "active ad has", "active ads have")
            ),
            detail: format!(
                "In the 30 days before the Deep check, {}{} with {} spend had the ad strength Poor: {}.",
                counted(poor.len(), "ad", "ads"),
                campaign.map_or_else(String::new, |campaign| format!(" in {campaign}")),
                money.of(spend),
                names(&poor)
            ),
            subjects: subjects(&poor),
            spend,
        });
    }
    let fixed = usize::from(
        findings
            .first()
            .is_some_and(|first| first.key.starts_with("tracking")),
    );
    findings[fixed..].sort_by(|a, b| b.spend.total_cmp(&a.spend));
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, clicks: f64, conversions: f64, cost: f64) -> AuditRow {
        AuditRow {
            name: name.into(),
            counts: Counts {
                impressions: clicks * 20.0,
                clicks,
                conversions,
                cost,
                value: 0.0,
            },
            ..Default::default()
        }
    }

    fn data(conversions: f64, audit: Audit) -> Data {
        Data {
            currency: "USD".into(),
            reporting: Counts {
                impressions: 40000.0,
                clicks: 2000.0,
                conversions,
                cost: 4000.0,
                value: 0.0,
            },
            audit,
        }
    }

    #[test]
    fn wasted_search_terms_need_enough_spend_to_be_a_finding() {
        let found = detect(
            &data(
                100.0,
                Audit {
                    conversion_actions: Some(2),
                    search_terms: vec![
                        row("free dentist", 60.0, 0.0, 90.0),
                        row("dentist jobs", 50.0, 0.0, 70.0),
                        row("cheap one", 10.0, 0.0, 12.0),
                        row("good term", 80.0, 9.0, 150.0),
                    ],
                    ..Default::default()
                },
            ),
            true,
            "account",
            Some("Search - Emergency"),
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].key, "search-terms-no-conversions");
        assert_eq!(
            found[0].title,
            "2 search terms had spend and no conversions"
        );
        assert_eq!(found[0].detail, "In the 30 days before the Deep check, Search - Emergency had $160 spend and 0 conversions on 2 search terms, each with spend at or above the account cost per conversion of $40.00: free dentist, dentist jobs.");
        assert_eq!(found[0].subjects, ["free dentist", "dentist jobs"]);
        let none = detect(
            &data(
                100.0,
                Audit {
                    conversion_actions: Some(2),
                    search_terms: vec![row("free dentist", 60.0, 0.0, 70.0)],
                    ..Default::default()
                },
            ),
            true,
            "account",
            None,
        );
        assert_eq!(none, vec![]);
    }

    #[test]
    fn one_wasted_keyword_and_one_poor_ad_are_named_in_the_singular() {
        let mut poor = row("Emergency dentist | Open today", 30.0, 1.0, 40.0);
        poor.strength = "POOR".into();
        let found = detect(
            &data(
                100.0,
                Audit {
                    conversion_actions: Some(2),
                    keywords: vec![row("dentist near me", 90.0, 0.0, 120.0)],
                    ads: vec![poor],
                    ..Default::default()
                },
            ),
            true,
            "account",
            Some("Search - Emergency"),
        );
        assert_eq!(
            found
                .iter()
                .map(|finding| (finding.title.as_str(), finding.detail.as_str()))
                .collect::<Vec<_>>(),
            [
                (
                    "1 keyword had spend and no conversions",
                    "In the 30 days before the Deep check, Search - Emergency had $120 spend and 0 conversions on 1 keyword, each with spend at or above the account cost per conversion of $40.00: dentist near me."
                ),
                (
                    "1 active ad has poor ad strength",
                    "In the 30 days before the Deep check, 1 ad in Search - Emergency with $40.00 spend had the ad strength Poor: Emergency dentist | Open today."
                ),
            ]
        );
    }

    #[test]
    fn campaign_rules_use_the_account_cost_per_conversion() {
        let mut efficient = row("Search - Brand", 500.0, 40.0, 800.0);
        efficient.lost_to_budget = Some(0.22);
        let found = detect(
            &data(
                100.0,
                Audit {
                    conversion_actions: Some(1),
                    campaigns: vec![
                        efficient,
                        row("Display - Remarketing", 300.0, 5.0, 500.0),
                        row("Local", 200.0, 4.0, 400.0),
                        row("Video", 400.0, 0.0, 170.0),
                        row("Tiny", 5.0, 0.0, 85.0),
                    ],
                    ..Default::default()
                },
            ),
            true,
            "account",
            None,
        );
        let keys = found
            .iter()
            .map(|finding| finding.key.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            keys,
            [
                "budget-limited:Search - Brand",
                "campaign-cost-high:Display - Remarketing",
                "campaign-cost-high:Video"
            ]
        );
        assert_eq!(found[2].detail, "Video spent $170 for 0 conversions (no cost per conversion). The account cost per conversion is $40.00. It had 4% of the spend and 0% of the conversions.");
        assert_eq!(found[0].detail, "Search - Brand lost 22% of its estimated eligible Search impressions to budget. Its cost per conversion is $20.00, at or below the account value of $40.00, with 40 conversions.");
        assert_eq!(found[1].detail, "Display - Remarketing spent $500 for 5 conversions ($100 per conversion). The account cost per conversion is $40.00. It had 12% of the spend and 5% of the conversions.");
    }

    #[test]
    fn an_account_without_conversions_has_a_tracking_finding_and_click_cost_findings() {
        let untracked = data(
            0.0,
            Audit {
                conversion_actions: Some(0),
                campaigns: vec![
                    row("Porta", 40.0, 0.0, 400.0),
                    row("Pizza", 1900.0, 0.0, 3600.0),
                ],
                ..Default::default()
            },
        );
        assert_eq!(tracking(&untracked), "not set up");
        let found = detect(&untracked, false, "account", None);
        assert_eq!(found[0].key, "tracking-not-set-up");
        assert_eq!(found[0].detail, "The Ad Account has no active conversion action. $4,000 spend and 2,000 clicks cannot be judged by results.");
        assert_eq!(found[1].key, "campaign-click-cost-high:Porta");
        assert_eq!(
            found[1].detail,
            "Porta paid $10.00 per click for 40 clicks. The account CPC is $2.00."
        );
        let silent = data(
            0.0,
            Audit {
                conversion_actions: Some(3),
                ..Default::default()
            },
        );
        assert_eq!(tracking(&silent), "no results");
        assert_eq!(
            detect(&silent, false, "account", None)[0].key,
            "tracking-no-results"
        );
        assert_eq!(tracking(&data(9.0, Audit::default())), "few results");
        assert_eq!(tracking(&data(29.0, Audit::default())), "uncertain");
    }

    fn valued(mut row: AuditRow, value: f64) -> AuditRow {
        row.counts.value = value;
        row
    }

    fn compared(name: &str, now: (f64, f64, f64), before: (f64, f64, f64)) -> AuditRow {
        AuditRow {
            previous: Some(row(name, before.0, before.1, before.2).counts),
            ..row(name, now.0, now.1, now.2)
        }
    }

    #[test]
    fn a_campaign_with_more_clicks_or_dearer_clicks_and_no_more_conversions_is_a_finding() {
        let found = detect(
            &data(
                100.0,
                Audit {
                    conversion_actions: Some(1),
                    campaigns: vec![
                        compared("Broad", (2580.0, 124.0, 4556.0), (1493.0, 119.0, 2984.0)),
                        compared("Implants", (1230.0, 90.0, 5700.0), (1500.0, 90.0, 4500.0)),
                        compared("Steady", (1000.0, 50.0, 2000.0), (1000.0, 50.0, 2000.0)),
                        compared("Growing", (1500.0, 75.0, 3000.0), (1000.0, 50.0, 2000.0)),
                    ],
                    ..Default::default()
                },
            ),
            true,
            "Ad Account",
            None,
        );
        let details = found
            .iter()
            .filter(|finding| !finding.key.starts_with("campaign-cost-high"))
            .map(|finding| (finding.key.as_str(), finding.detail.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            details,
            [
                ("click-cost-rising:Implants", "In the 30 days before the Deep check, Implants paid $4.63 per click against $3.00 in the previous 30 days (up 54%), with 90 conversions against 90, and its cost per conversion rose from $50.00 to $63.33."),
                ("traffic-without-results:Broad", "In the 30 days before the Deep check, Broad had 2,580 clicks against 1,493 in the previous 30 days (up 73%), but 124 conversions against 119, and its cost per conversion rose from $25.08 to $36.74."),
            ]
        );
    }

    #[test]
    fn a_location_with_much_of_the_spend_and_a_high_cost_per_conversion_is_a_finding() {
        let found = detect(
            &data(
                100.0,
                Audit {
                    conversion_actions: Some(1),
                    locations: vec![
                        row("Austin, Texas", 1400.0, 12.0, 2800.0),
                        row("Round Rock, Texas", 600.0, 88.0, 1200.0),
                    ],
                    ..Default::default()
                },
            ),
            true,
            "Ad Account",
            None,
        );
        assert_eq!(
            found.iter().map(|finding| finding.detail.as_str()).collect::<Vec<_>>(),
            ["In the 30 days before the Deep check, Austin, Texas had 70% of the spend and 12% of the conversions: $2,800 for 12 conversions ($233 per conversion). The Ad Account cost per conversion is $40.00."]
        );
    }

    #[test]
    fn an_account_judged_by_value_names_the_campaign_with_a_low_return_instead_of_its_cost() {
        let mut shop = data(
            31.0,
            Audit {
                conversion_actions: Some(1),
                campaigns: vec![
                    valued(row("Search - Brand", 300.0, 18.0, 600.0), 8100.0),
                    valued(row("Shopping - Generic", 800.0, 6.0, 3400.0), 1900.0),
                ],
                ..Default::default()
            },
        );
        shop.reporting.value = 10000.0;
        let found = detect(&shop, true, "Ad Account", None);
        assert_eq!(
            found.iter().map(|finding| finding.detail.as_str()).collect::<Vec<_>>(),
            ["Shopping - Generic spent $3,400 for $1,900 conversion value, a return on ad spend of 0.56. The Ad Account return on ad spend is 2.50. It had 85% of the spend and 19% of the conversion value."]
        );
    }

    #[test]
    fn a_landing_page_that_converts_below_the_account_names_the_page_that_converts_better() {
        let found = detect(
            &data(
                168.0,
                Audit {
                    conversion_actions: Some(1),
                    landing_pages: vec![
                        row("https://x.example/offers", 2300.0, 46.0, 3000.0),
                        row("https://x.example/emergency", 550.0, 50.0, 750.0),
                    ],
                    ..Default::default()
                },
            ),
            true,
            "account",
            None,
        );
        assert_eq!(
            found.iter().map(|finding| finding.detail.as_str()).collect::<Vec<_>>(),
            ["https://x.example/offers had 2,300 clicks and 46 conversions, a conversion rate of 2.00%. The account conversion rate is 8.40%. https://x.example/emergency converted 9.09% of its 550 clicks."]
        );
    }

    #[test]
    fn a_campaign_under_the_break_even_of_the_profile_is_a_finding_in_place_of_the_relative_rules()
    {
        let mut shop = data(
            31.0,
            Audit {
                conversion_actions: Some(1),
                campaigns: vec![
                    valued(row("Shopping - Garden Tools", 900.0, 9.0, 2700.0), 7020.0),
                    valued(row("Search - Brand", 200.0, 8.0, 420.0), 3780.0),
                ],
                economics: crate::monitoring::economics::Economics {
                    margin: Some(0.25),
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        shop.reporting.value = 10800.0;
        shop.reporting.cost = 3120.0;
        assert_eq!(
            detect(&shop, true, "Ad Account", None)
                .iter()
                .map(|finding| finding.detail.as_str())
                .collect::<Vec<_>>(),
            ["Shopping - Garden Tools spent $2,700 for $7,020 conversion value, a return on ad spend of 2.60. With a margin of 25%, a return below 4.00 does not cover the ad cost. It had 87% of the spend."]
        );
        let mut leads = data(
            90.0,
            Audit {
                conversion_actions: Some(1),
                campaigns: vec![row("Search - Generic Windows", 3600.0, 60.0, 9000.0)],
                economics: crate::monitoring::economics::Economics {
                    margin: Some(0.4),
                    customer_value: Some(1500.0),
                    close_rate: Some(0.2),
                    target_return: None,
                },
                ..Default::default()
            },
        );
        leads.reporting.cost = 9000.0;
        assert_eq!(
            detect(&leads, true, "Ad Account", None)
                .iter()
                .map(|finding| finding.detail.as_str())
                .collect::<Vec<_>>(),
            ["Search - Generic Windows paid $150 per conversion for 60 conversions. At the value of a conversion in the Ad Account profile, a cost above $120 per conversion loses money. It had 100% of the spend."]
        );
    }
}
