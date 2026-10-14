use serde::{Deserialize, Serialize};

use super::{
    audit::{Audit, AuditRow, Counts, Data},
    metrics::{english_money, english_number},
    tracking::MIN_CONVERSIONS,
};

const UNCERTAIN_CONVERSIONS: f64 = 30.0;
const CLICKS: f64 = 30.0;
const PAGE_CLICKS: f64 = 100.0;
const FAR: f64 = 2.0;
const SHARE: f64 = 0.02;
const LISTED: usize = 5;
const PROVEN_CONVERSIONS: f64 = 5.0;
const VERY_FAR: f64 = 4.0;

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

fn count(value: f64) -> String {
    english_number(value, if value.fract() == 0.0 { 0 } else { 2 })
}

fn ratio(numerator: f64, denominator: f64) -> Option<f64> {
    (denominator > 0.0).then(|| numerator / denominator)
}

pub fn detect(data: &Data, judge_by_conversions: bool, account_label: &str) -> Vec<Finding> {
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
        for (key, noun, rows) in [
            (
                "search-terms-no-conversions",
                "search terms",
                &audit.search_terms,
            ),
            ("keywords-no-conversions", "keywords", &audit.keywords),
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
                    title: format!("{} {noun} had spend and no conversions", rows.len()),
                    detail: format!(
                        "{} spend and 0 conversions on {} {noun}, each with spend above the {account_label} cost per conversion of {}: {}.",
                        money.of(spend),
                        rows.len(),
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
                            "{} lost {}% of its search impressions to budget. Its cost per conversion is {}, at or below the {account_label} value of {}, with {} conversions.",
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
            if costly(&row.counts, account) && material(row.counts.cost) {
                findings.push(Finding {
                    key: format!("campaign-cost-high:{}", row.name),
                    title: format!("{} has a high cost per conversion", row.name),
                    detail: format!(
                        "{} spent {} for {} conversions ({}). The {account_label} cost per conversion is {}.",
                        row.name,
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
        for row in &audit.landing_pages {
            let page_rate = ratio(row.counts.conversions, row.counts.clicks).unwrap_or_default();
            if row.counts.clicks >= PAGE_CLICKS
                && rate > 0.0
                && page_rate <= rate / FAR
                && material(row.counts.cost)
            {
                findings.push(Finding {
                    key: format!("landing-page-low-rate:{}", row.name),
                    title: format!("The landing page {} converts below the {account_label} rate", row.name),
                    detail: format!(
                        "{} had {} clicks and {} conversions, a conversion rate of {}%. The {account_label} conversion rate is {}%.",
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
                            "{} lost {}% of its search impressions to budget. Its CTR is {}%, at or above the {account_label} CTR of {}%, and its CPC is not above the {account_label} CPC.",
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
            title: format!("{} active ads have poor ad strength", poor.len()),
            detail: format!(
                "{} ads with {} spend have the ad strength Poor: {}.",
                poor.len(),
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
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].key, "search-terms-no-conversions");
        assert_eq!(
            found[0].title,
            "2 search terms had spend and no conversions"
        );
        assert_eq!(found[0].detail, "$160 spend and 0 conversions on 2 search terms, each with spend above the account cost per conversion of $40.00: free dentist, dentist jobs.");
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
        );
        assert_eq!(none, vec![]);
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
        assert_eq!(found[2].detail, "Video spent $170 for 0 conversions (no cost per conversion). The account cost per conversion is $40.00.");
        assert_eq!(found[0].detail, "Search - Brand lost 22% of its search impressions to budget. Its cost per conversion is $20.00, at or below the account value of $40.00, with 40 conversions.");
        assert_eq!(found[1].detail, "Display - Remarketing spent $500 for 5 conversions ($100 per conversion). The account cost per conversion is $40.00.");
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
        let found = detect(&untracked, false, "account");
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
            detect(&silent, false, "account")[0].key,
            "tracking-no-results"
        );
        assert_eq!(tracking(&data(9.0, Audit::default())), "few results");
        assert_eq!(tracking(&data(29.0, Audit::default())), "uncertain");
    }
}
