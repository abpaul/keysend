use std::collections::BTreeMap;

use chrono::{Datelike, Days, NaiveDate, Weekday};

use super::{
    audit::{Audit, AuditRow, Counts, Data},
    deep_check::{DeepCheck, Finding},
    detection::{CampaignMovement, Detection},
    findings,
    metrics::{self, Metrics, Window, WindowMetrics},
    tracking,
    update::{CampaignLine, Update},
};
use crate::workspace::{
    history::{self, day_number, AccountChange},
    monitoring::{
        Detected, Recommendation, RecommendationState, RecommendationWithFinding, SearchShareDay,
        SearchShareTotal, SearchShares, Share,
    },
};

pub const AD: &str = "4829153307";
pub const NAME: &str = "Harbour Dental";
pub const CURRENCY: &str = "USD";
pub const HISTORY_DAYS: u64 = 112;
pub const BREAKDOWN_DAYS: i64 = 56;
pub const UPDATES: i64 = 31;
pub const EMERGENCY: i64 = 701;
pub const IMPLANTS: i64 = 702;
pub const BRAND: i64 = 703;
pub const REMARKETING: i64 = 704;
pub const CONVERSION_ACTIONS: u32 = 3;

pub struct Campaign {
    pub id: i64,
    pub name: &'static str,
    pub channel: &'static str,
    pub budget: i64,
    pub spend: f64,
    pub click_cost: f64,
    pub conversion_cost: f64,
    pub search_share: Option<(f64, f64, f64)>,
}

pub const CAMPAIGNS: [Campaign; 4] = [
    Campaign {
        id: EMERGENCY,
        name: "Search - Emergency",
        channel: "SEARCH",
        budget: 340,
        spend: 330.0,
        click_cost: 2.6,
        conversion_cost: 46.0,
        search_share: Some((0.52, 0.31, 0.17)),
    },
    Campaign {
        id: IMPLANTS,
        name: "Search - Implants",
        channel: "SEARCH",
        budget: 160,
        spend: 135.0,
        click_cost: 3.4,
        conversion_cost: 92.0,
        search_share: Some((0.64, 0.06, 0.30)),
    },
    Campaign {
        id: BRAND,
        name: "Search - Brand",
        channel: "SEARCH",
        budget: 120,
        spend: 88.0,
        click_cost: 0.9,
        conversion_cost: 21.0,
        search_share: Some((0.91, 0.02, 0.07)),
    },
    Campaign {
        id: REMARKETING,
        name: "Display - Remarketing",
        channel: "DISPLAY",
        budget: 20,
        spend: 38.0,
        click_cost: 0.55,
        conversion_cost: 27.0,
        search_share: None,
    },
];

const fn date(month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, month, day).unwrap()
}

pub const END: NaiveDate = date(10, 11);
const IMPLANTS_BUDGET_UP: NaiveDate = date(9, 14);
const UNUSUAL_SPEND: NaiveDate = date(9, 23);
const BID_STRATEGY_CHANGE: NaiveDate = date(10, 4);
const REMARKETING_BUDGET_DOWN: NaiveDate = date(10, 8);

pub fn today() -> i64 {
    day_number(END) + 1
}

pub fn noise(seed: u64) -> f64 {
    (seed.wrapping_mul(2_654_435_761) >> 7) as f64 % 1000.0 / 500.0 - 1.0
}

pub fn campaigns() -> Vec<history::Campaign> {
    CAMPAIGNS
        .iter()
        .map(|campaign| history::Campaign {
            id: campaign.id,
            name: campaign.name.into(),
            status: "ENABLED".into(),
            channel_type: campaign.channel.into(),
            bidding_strategy: "MAXIMIZE_CONVERSIONS".into(),
            budget_micros: Some(campaign.budget * 1_000_000),
        })
        .collect()
}

pub fn history() -> Vec<history::CampaignDay> {
    let mut rows = Vec::new();
    for offset in 0..HISTORY_DAYS {
        let day = END - Days::new(offset);
        let weekday = match day.weekday() {
            Weekday::Sat => 0.72,
            Weekday::Sun => 0.64,
            Weekday::Mon => 1.08,
            _ => 1.0,
        };
        for campaign in &CAMPAIGNS {
            let seed = offset * 31 + campaign.id as u64 * 7;
            let mut spend = campaign.spend * weekday * (1.0 + 0.06 * noise(seed));
            let mut conversion_cost = campaign.conversion_cost * (1.0 + 0.12 * noise(seed + 3));
            if campaign.id == EMERGENCY && day == UNUSUAL_SPEND {
                spend *= 2.2;
            }
            if campaign.id == EMERGENCY && day >= BID_STRATEGY_CHANGE {
                conversion_cost *= 0.8;
            }
            if campaign.id == REMARKETING && day >= REMARKETING_BUDGET_DOWN {
                spend *= 0.45;
                conversion_cost *= 1.5;
            }
            for (device, share, rate) in [(1_i64, 0.66, 0.96), (2, 0.34, 1.08)] {
                let cost = (spend * share * 100.0).round() / 100.0;
                let clicks = (cost / campaign.click_cost).round();
                rows.push(history::CampaignDay {
                    day,
                    campaign_id: campaign.id,
                    device,
                    counts: Counts {
                        impressions: (clicks * 14.0).round(),
                        clicks,
                        cost,
                        conversions: (cost / conversion_cost * rate).round(),
                        value: 0.0,
                    },
                });
            }
        }
    }
    rows
}

pub fn days() -> Vec<metrics::CampaignDay> {
    let mut totals: BTreeMap<(i64, i64), (i64, Metrics)> = BTreeMap::new();
    for row in history() {
        let (micros, total) = totals
            .entry((day_number(row.day), row.campaign_id))
            .or_insert_with(|| {
                (
                    0,
                    Metrics {
                        conversions: Some(0.0),
                        conversion_value: Some(0.0),
                        ..Metrics::default()
                    },
                )
            });
        *micros += (row.counts.cost * 1_000_000.0).round() as i64;
        total.add(&Metrics {
            spend: 0.0,
            clicks: row.counts.clicks,
            impressions: row.counts.impressions,
            conversions: Some(row.counts.conversions),
            conversion_value: Some(row.counts.value),
        });
    }
    totals
        .into_iter()
        .map(|((day, campaign_id), (micros, mut metrics))| {
            metrics.spend = micros as f64 / 1_000_000.0;
            metrics::CampaignDay {
                day,
                campaign_id,
                metrics,
            }
        })
        .collect()
}

pub fn changes() -> Vec<AccountChange> {
    let change = |index: u32, day: NaiveDate, campaign: i64, kind: &str, old: &str, new: &str| {
        AccountChange {
            changed_at: format!("{day} 10:00:00"),
            resource_name: format!("customers/{AD}/changeEvents/{index}"),
            resource_type: kind.into(),
            operation: "UPDATE".into(),
            campaign_id: Some(campaign),
            changed_fields: String::new(),
            old_value: old.into(),
            new_value: new.into(),
        }
    };
    vec![
        change(
            3,
            IMPLANTS_BUDGET_UP,
            IMPLANTS,
            "CAMPAIGN_BUDGET",
            r#"{"campaignBudget":{"amountMicros":"140000000"}}"#,
            r#"{"campaignBudget":{"amountMicros":"160000000"}}"#,
        ),
        change(
            1,
            BID_STRATEGY_CHANGE,
            EMERGENCY,
            "CAMPAIGN",
            r#"{"campaign":{"biddingStrategyType":"MAXIMIZE_CLICKS"}}"#,
            r#"{"campaign":{"biddingStrategyType":"MAXIMIZE_CONVERSIONS"}}"#,
        ),
        change(
            2,
            REMARKETING_BUDGET_DOWN,
            REMARKETING,
            "CAMPAIGN_BUDGET",
            r#"{"campaignBudget":{"amountMicros":"45000000"}}"#,
            r#"{"campaignBudget":{"amountMicros":"20000000"}}"#,
        ),
    ]
}

fn row(name: &str, cost: f64, clicks: f64, conversions: f64) -> AuditRow {
    AuditRow {
        name: name.into(),
        counts: Counts {
            impressions: clicks * 12.0,
            clicks,
            conversions,
            cost,
            value: 0.0,
        },
        ..Default::default()
    }
}

fn ads(names: [&str; 2]) -> Vec<AuditRow> {
    names
        .into_iter()
        .zip([(9001, 412.0, "GOOD"), (9002, 188.0, "AVERAGE")])
        .map(|(name, (id, clicks, strength))| AuditRow {
            id: Some(id),
            strength: strength.into(),
            ..row(name, clicks * 2.4, clicks, (clicks / 18.0).floor())
        })
        .collect()
}

pub fn audits() -> BTreeMap<i64, Audit> {
    let campaign = |campaign: &Campaign, cost: f64, clicks: f64, conversions: f64| AuditRow {
        id: Some(campaign.id),
        ..row(campaign.name, cost, clicks, conversions)
    };
    let audit = |campaign: AuditRow, terms, keywords, pages, ads| Audit {
        conversion_actions: Some(CONVERSION_ACTIONS),
        campaigns: vec![campaign],
        search_terms: terms,
        keywords,
        landing_pages: pages,
        ads,
        changes: vec![],
    };
    BTreeMap::from([
        (
            EMERGENCY,
            audit(
                campaign(&CAMPAIGNS[0], 9120.0, 3510.0, 214.0),
                vec![
                    row("emergency dentist near me", 2612.0, 1004.0, 71.0),
                    row("tooth pain dentist today", 1344.0, 517.0, 33.0),
                    row("emergency dentist", 1180.0, 454.0, 29.0),
                    row("dentist open saturday", 641.0, 247.0, 15.0),
                    row("free dental care", 296.0, 121.0, 0.0),
                ],
                vec![
                    row("emergency dentist", 4520.0, 1740.0, 112.0),
                    row("dentist open now", 2260.0, 870.0, 41.0),
                    row("tooth pain", 1180.0, 454.0, 24.0),
                    row("24 hour dentist", 890.0, 342.0, 31.0),
                ],
                vec![
                    row(
                        "https://harbourdental.example/emergency",
                        5210.0,
                        2004.0,
                        158.0,
                    ),
                    row("https://harbourdental.example/", 3380.0, 1300.0, 47.0),
                    row("https://harbourdental.example/prices", 530.0, 206.0, 9.0),
                ],
                ads([
                    "Emergency Dentist | Open Today | Harbour Dental",
                    "Same-Day Appointments | Call Harbour Dental",
                ]),
            ),
        ),
        (
            IMPLANTS,
            audit(
                campaign(&CAMPAIGNS[1], 3760.0, 1106.0, 41.0),
                vec![
                    row("dental implants cost", 1420.0, 418.0, 19.0),
                    row("dental implants near me", 1105.0, 325.0, 16.0),
                    row("dental implants jobs", 186.0, 74.0, 0.0),
                    row("all on 4 implants", 164.0, 48.0, 2.0),
                ],
                vec![
                    row("dental implants", 2540.0, 747.0, 27.0),
                    row("tooth implant cost", 820.0, 241.0, 10.0),
                    row("implant dentist", 400.0, 118.0, 4.0),
                ],
                vec![
                    row(
                        "https://harbourdental.example/implants",
                        3210.0,
                        944.0,
                        37.0,
                    ),
                    row("https://harbourdental.example/", 550.0, 162.0, 4.0),
                ],
                ads([
                    "Dental Implants | Fixed Price | Harbour Dental",
                    "Implants From One Visit | Book a Consultation",
                ]),
            ),
        ),
        (
            BRAND,
            audit(
                campaign(&CAMPAIGNS[2], 2440.0, 2711.0, 118.0),
                vec![
                    row("harbour dental", 1810.0, 2011.0, 92.0),
                    row("harbour dental opening hours", 430.0, 478.0, 19.0),
                    row("harbour dental reviews", 200.0, 222.0, 7.0),
                ],
                vec![
                    row("harbour dental", 2240.0, 2489.0, 111.0),
                    row("harbour dentist", 200.0, 222.0, 7.0),
                ],
                vec![row("https://harbourdental.example/", 2440.0, 2711.0, 118.0)],
                ads([
                    "Harbour Dental | Book Online | Open 6 Days",
                    "Harbour Dental | New Patients Welcome",
                ]),
            ),
        ),
        (
            REMARKETING,
            audit(
                campaign(&CAMPAIGNS[3], 960.0, 1745.0, 34.0),
                vec![],
                vec![],
                vec![row(
                    "https://harbourdental.example/book",
                    960.0,
                    1745.0,
                    34.0,
                )],
                ads([
                    "Responsive display ad in ad group Past visitors",
                    "Responsive display ad in ad group Booking started",
                ]),
            ),
        ),
    ])
}

pub fn deep_check(day: i64) -> (DeepCheck, Vec<Recommendation>) {
    let by_campaign = audits();
    let finding =
        |campaign: i64, key: &str, title: &str, detail: &str, subject: &str, spend| Finding {
            campaign_id: Some(campaign),
            rule: findings::Finding {
                key: key.into(),
                title: title.into(),
                detail: detail.into(),
                subjects: vec![subject.into()],
                spend,
            },
        };
    let findings = vec![
        finding(
            EMERGENCY,
            "budget-limited:Search - Emergency",
            "Search - Emergency is limited by its budget",
            "Search - Emergency lost 31% of its search impressions to budget. Its cost per conversion is $42.62, below the Ad Account value of $52.01, with 214 conversions.",
            "Search - Emergency",
            9120.0,
        ),
        finding(
            IMPLANTS,
            "wasted-term:702:dental implants jobs",
            "Wasted search term",
            "dental implants jobs: $186 spend, 74 clicks, 0 conversions in the last 30 days.",
            "dental implants jobs",
            186.0,
        ),
        finding(
            EMERGENCY,
            "landing-page-low-rate:https://harbourdental.example/",
            "A landing page of Search - Emergency has a high cost",
            "https://harbourdental.example/: $3,380 spend, 1,300 clicks, 47 conversions. The emergency page gets 158 conversions from 2,004 clicks.",
            "https://harbourdental.example/",
            3380.0,
        ),
    ];
    let actions = [
        (
            "Increase the daily budget of Search - Emergency.",
            "The ads miss searches when the daily budget is used up, and this Campaign has the lowest cost per conversion of the Search Campaigns that are not for your own name.",
        ),
        (
            "Add dental implants jobs as a negative keyword in Search - Implants.",
            "People who look for a job are not patients. This search term spends without conversions.",
        ),
        (
            "Send the ads in Search - Emergency to the emergency page, not to the home page.",
            "The home page needs 28 clicks for one conversion. The emergency page needs 13.",
        ),
    ];
    let recommendations = findings
        .iter()
        .zip(actions)
        .map(|(finding, (action, reason))| Recommendation {
            finding_key: finding.rule.key.clone(),
            action: action.into(),
            reason: reason.into(),
            evidence: finding.rule.detail.clone(),
            campaign_id: finding.campaign_id,
            state: RecommendationState::Open,
        })
        .collect();
    (
        DeepCheck {
            day,
            data: Data {
                currency: CURRENCY.into(),
                reporting: Counts {
                    impressions: 127_000.0,
                    clicks: 9_072.0,
                    cost: 16_280.0,
                    conversions: 407.0,
                    value: 0.0,
                },
                audit: Audit {
                    conversion_actions: Some(CONVERSION_ACTIONS),
                    campaigns: by_campaign
                        .values()
                        .flat_map(|audit| audit.campaigns.clone())
                        .collect(),
                    ..Default::default()
                },
            },
            by_campaign,
            findings,
            judge_by_conversions: true,
            recommendations_generated: true,
        },
        recommendations,
    )
}

pub fn recommendations() -> Vec<RecommendationWithFinding> {
    let (check, recommendations) = deep_check(day_number(END));
    check
        .findings
        .into_iter()
        .zip(recommendations)
        .map(|(finding, recommendation)| RecommendationWithFinding {
            recommendation,
            finding: Some(Detected {
                title: finding.rule.title,
                spend: finding.rule.spend,
            }),
        })
        .collect()
}

fn dollars(value: f64) -> String {
    format!("${}", metrics::english_number(value.round(), 0))
}

const QUIET: &str = "Nothing notable changed in the last 7 days. Spend and conversions are inside their normal range.";

pub fn update(days: &[metrics::CampaignDay], offset: i64) -> Update {
    let day = today() - offset;
    let end = day - 1;
    let (_, judge) = tracking::of_days(days, end, None, None);
    let of = |campaign| metrics::window_metrics(days, end, Window::SevenDays, campaign, judge);
    let metrics = of(None);
    let conversions = |metrics: &WindowMetrics| {
        (
            metrics.previous.conversions.unwrap_or_default().round(),
            metrics.current.conversions.unwrap_or_default().round(),
        )
    };
    let (summary, highlight, lines) = match offset {
        0 => {
            let (emergency, remarketing) = (of(Some(EMERGENCY)), of(Some(REMARKETING)));
            let (before, now) = conversions(&metrics);
            let highlight = format!("conversions rose from {before} to {now}");
            let (emergency_before, emergency_now) = conversions(&emergency);
            let (remarketing_before, remarketing_now) = conversions(&remarketing);
            (
                format!(
                    "Spend was steady this week at {}, and {highlight}. Most of the gain came from Search - Emergency after its bid strategy changed on 4 October. Display - Remarketing is the one to watch.",
                    dollars(metrics.current.spend)
                ),
                Some(highlight),
                vec![
                    CampaignLine {
                        campaign_id: EMERGENCY,
                        text: format!(
                            "{emergency_now} conversions, up from {emergency_before}, at ${:.2} each.",
                            emergency.current.spend / emergency_now
                        ),
                                            cause: None,
                    },
                    CampaignLine {
                        campaign_id: REMARKETING,
                        text: format!(
                            "conversions fell from {remarketing_before} to {remarketing_now} after the daily budget went from $45 to $20 on 8 October. The count is small, so this can be chance."
                        ),
                                            cause: None,
                    },
                ],
            )
        }
        4 => (
            format!(
                "The daily budget of Display - Remarketing went from $45 to $20 today. Spend in the last 7 days was {}.",
                dollars(metrics.current.spend)
            ),
            None,
            vec![],
        ),
        8 => (
            "The bid strategy of Search - Emergency changed from maximize clicks to maximize conversions today. Expect about a week before its numbers settle.".to_owned(),
            None,
            vec![],
        ),
        19 => (
            "Spend on 23 September was above the usual range for a Wednesday. Search - Emergency took most of it, and its conversions rose with it.".to_owned(),
            None,
            vec![],
        ),
        _ => (QUIET.to_owned(), None, vec![]),
    };
    Update {
        day,
        summary,
        highlight,
        cause: None,
        detection: Detection {
            day,
            currency: CURRENCY.into(),
            metrics: metrics.clone(),
            movements: vec![],
            yesterday: vec![],
            campaigns: lines
                .iter()
                .filter_map(|line| {
                    let campaign = CAMPAIGNS
                        .iter()
                        .find(|campaign| campaign.id == line.campaign_id)?;
                    Some(CampaignMovement {
                        campaign_id: campaign.id,
                        name: campaign.name.into(),
                        metrics: metrics.clone(),
                        movements: vec![],
                        reasons: vec![],
                        account_changes: vec![],
                        follow_ups: vec![],
                    })
                })
                .collect(),
            judge_by_conversions: true,
            causes: vec![],
        },
        lines,
        metrics,
        not_useful: false,
        written_without_ai: false,
        more_campaigns: 0,
    }
}

pub fn search_share_days() -> Vec<SearchShareDay> {
    let last = day_number(END);
    let mut days = Vec::new();
    for offset in 0..BREAKDOWN_DAYS {
        for campaign in &CAMPAIGNS {
            let Some((shown, budget, rank)) = campaign.search_share else {
                continue;
            };
            let wave = noise((offset * 13 + campaign.id) as u64) * 0.03;
            days.push(SearchShareDay {
                day: last - offset,
                campaign_id: campaign.id,
                impressions: 900,
                shown: Share::Exact(shown + wave),
                lost_budget: Share::Exact(budget - wave / 2.0),
                lost_rank: Share::Exact(rank - wave / 2.0),
            });
        }
    }
    days
}

pub fn search_share_totals(start: i64, end: i64) -> Vec<SearchShareTotal> {
    let mut totals: BTreeMap<i64, (i64, f64, f64, f64, f64)> = BTreeMap::new();
    for day in search_share_days()
        .into_iter()
        .filter(|day| (start..=end).contains(&day.day))
    {
        let (Share::Exact(shown), Share::Exact(budget), Share::Exact(rank)) =
            (day.shown, day.lost_budget, day.lost_rank)
        else {
            continue;
        };
        let eligible = day.impressions as f64 / shown;
        let total = totals.entry(day.campaign_id).or_default();
        total.0 += 1;
        total.1 += day.impressions as f64;
        total.2 += eligible;
        total.3 += budget * eligible;
        total.4 += rank * eligible;
    }
    totals
        .into_iter()
        .map(
            |(campaign_id, (days, impressions, eligible, budget, rank))| SearchShareTotal {
                campaign_id,
                shares: Some(SearchShares {
                    shown: impressions / eligible,
                    lost_budget: budget / eligible,
                    lost_rank: rank / eligible,
                }),
                limit_days: 0,
                exact_days: days,
                days,
            },
        )
        .collect()
}
