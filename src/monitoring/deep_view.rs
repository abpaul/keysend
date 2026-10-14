use std::collections::BTreeMap;

use loco_rs::Result;
use serde::Serialize;

use super::{
    audit::{Audit, AuditRow, Counts},
    charts::{meter, quality_sentence, score_bar},
    deep_check::AD_WITHOUT_NAME,
    format::ratio,
    view::STORED_UNNAMED_AD,
};
use crate::{
    language::{fill, say, say_amount, say_plural, Text},
    workspace::monitoring::KeywordScore,
};

pub const ROWS_SHOWN: usize = 10;

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Badge {
    pub kind: &'static str,
    pub text: String,
}

impl Badge {
    fn new(kind: &'static str, text: impl Into<String>) -> Option<Self> {
        Some(Self {
            kind,
            text: text.into(),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Drawn {
    Score(i64),
    Share { share: f64, mark: Option<f64> },
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Bar {
    pub svg: String,
    #[serde(skip)]
    pub drawn: Drawn,
}

fn bar(share: f64, mark: Option<f64>, label: &str) -> Option<Bar> {
    Some(Bar {
        svg: meter(share, mark, label),
        drawn: Drawn::Share {
            share: share.clamp(0.0, 1.0),
            mark: mark.map(|mark| mark.clamp(0.0, 1.0)),
        },
    })
}

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct Fact {
    pub label: &'static str,
    pub value: String,
    pub detail: String,
    pub bar: Option<Bar>,
    pub badge: Option<Badge>,
    pub wide: bool,
}

impl Fact {
    fn plain(label: &'static str, value: String) -> Self {
        Self {
            label,
            value,
            ..Default::default()
        }
    }

    fn missing(label: &'static str, why: String) -> Self {
        Self {
            label,
            detail: why,
            ..Default::default()
        }
    }
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Row {
    pub name: String,
    pub tags: Vec<&'static str>,
    pub badge: Option<Badge>,
    pub facts: Vec<Fact>,
}

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct Section {
    pub id: &'static str,
    pub title: &'static str,
    pub name: &'static str,
    pub url: bool,
    pub sentence: String,
    pub note: &'static str,
    pub columns: Vec<&'static str>,
    pub rows: Vec<Row>,
    pub key: String,
    pub about: Vec<&'static str>,
    pub terms: Vec<(&'static str, &'static str)>,
    pub actions: Vec<&'static str>,
    pub limitation: &'static str,
}

fn percent(text: &Text, rate: f64) -> String {
    format!(
        "{}{}",
        text.number(rate * 100.0, 1),
        text.written.format.percent_suffix
    )
}

fn clicks_n(text: &Text, value: f64) -> Result<String> {
    say_amount!(text, deep_clicks_n, value)
}

fn conversions_n(text: &Text, value: f64) -> Result<String> {
    say_amount!(text, deep_conversions_n, value)
}

fn impressions_n(text: &Text, value: f64) -> Result<String> {
    say_amount!(text, deep_impressions_n, value)
}

fn level(text: &Text, label: &'static str, bucket: Option<&str>) -> Fact {
    let w = &text.written;
    let badge = match bucket {
        Some("ABOVE_AVERAGE") => Badge::new("above", w.deep_level_above),
        Some("AVERAGE") => Badge::new("average", w.deep_level_average),
        Some("BELOW_AVERAGE") => Badge::new("below", w.deep_level_below),
        _ => None,
    };
    Fact {
        label,
        detail: if badge.is_some() {
            String::new()
        } else {
            w.deep_level_none.into()
        },
        badge,
        ..Default::default()
    }
}

fn match_type(text: &Text, value: &str) -> Option<&'static str> {
    let w = &text.written;
    match value {
        "EXACT" => Some(w.deep_match_exact),
        "PHRASE" => Some(w.deep_match_phrase),
        "BROAD" => Some(w.deep_match_broad),
        _ => None,
    }
}

const QUALITY_ID: &str = "keyword-quality";

pub fn quality_missing(text: &Text, sentence: String) -> Section {
    Section {
        id: QUALITY_ID,
        title: text.written.deep_quality_title,
        sentence,
        ..Default::default()
    }
}

pub fn quality(text: &Text, scores: &[KeywordScore], compared: bool) -> Result<Section> {
    let w = &text.written;
    let rows = scores
        .iter()
        .map(|row| {
            let quality = &row.quality;
            let facts = match quality.score {
                None => vec![Fact {
                    wide: true,
                    ..Fact::missing(w.deep_label_quality_score, w.deep_no_score.into())
                }],
                Some(score) => {
                    let (badge, detail) = match row.previous {
                        _ if !compared => (None, ""),
                        _ if !row.known_before => (None, w.deep_score_new),
                        None => (None, w.deep_score_none_before),
                        Some(before) if score > before => (
                            Badge::new(
                                "higher",
                                say!(text, deep_score_up, change = score - before)?,
                            ),
                            "",
                        ),
                        Some(before) if score < before => (
                            Badge::new(
                                "lower",
                                say!(text, deep_score_down, change = before - score)?,
                            ),
                            "",
                        ),
                        Some(_) => (Badge::new("equal", w.deep_score_same), ""),
                    };
                    vec![
                        Fact {
                            label: w.deep_label_quality_score,
                            value: say!(text, deep_score_value, score = score)?,
                            detail: detail.into(),
                            bar: Some(Bar {
                                svg: score_bar(text, score)?,
                                drawn: Drawn::Score(score),
                            }),
                            badge,
                            wide: false,
                        },
                        level(text, w.deep_label_ctr, quality.expected_ctr.as_deref()),
                        level(
                            text,
                            w.deep_label_relevance,
                            quality.ad_relevance.as_deref(),
                        ),
                        level(
                            text,
                            w.deep_label_landing_page,
                            quality.landing_page.as_deref(),
                        ),
                    ]
                }
            };
            Ok(Row {
                name: quality.keyword.clone(),
                tags: match_type(text, &quality.match_type).into_iter().collect(),
                badge: None,
                facts,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Section {
        id: QUALITY_ID,
        title: w.deep_quality_title,
        name: w.deep_name_keyword,
        sentence: quality_sentence(
            text,
            &scores
                .iter()
                .map(|row| row.quality.score)
                .collect::<Vec<_>>(),
        )?,
        note: if scores.iter().all(|row| row.quality.score.is_none()) {
            ""
        } else if compared {
            w.deep_score_changes
        } else {
            w.deep_first_scores
        },
        columns: vec![
            w.deep_label_quality_score,
            w.deep_label_ctr,
            w.deep_label_relevance,
            w.deep_label_landing_page,
        ],
        rows,
        about: vec![
            w.deep_quality_about_estimate,
            w.deep_quality_about_levels,
            w.deep_quality_about_auction,
        ],
        terms: vec![
            (w.deep_label_ctr, w.deep_term_ctr),
            (w.deep_label_relevance, w.deep_term_relevance),
            (w.deep_label_landing_page, w.deep_term_landing_page),
        ],
        actions: vec![
            w.deep_quality_action_ctr,
            w.deep_quality_action_relevance,
            w.deep_quality_action_landing_page,
        ],
        limitation: w.deep_quality_limit,
        ..Default::default()
    })
}

struct Scope<'a> {
    text: &'a Text,
    currency: &'a str,
    judged: bool,
    campaign: Option<&'a Counts>,
}

impl Scope<'_> {
    fn money(&self, value: f64) -> String {
        self.text.money(value, self.currency)
    }
}

fn largest(rows: &[AuditRow]) -> Vec<&AuditRow> {
    let mut rows = rows.iter().collect::<Vec<_>>();
    rows.sort_by(|a, b| {
        b.counts
            .cost
            .total_cmp(&a.counts.cost)
            .then(a.name.cmp(&b.name))
    });
    rows.truncate(ROWS_SHOWN);
    rows
}

fn spend(scope: &Scope, counts: &Counts, most: f64) -> Result<Fact> {
    let value = scope.money(counts.cost);
    Ok(Fact {
        label: scope.text.written.metric_spend,
        bar: bar(
            ratio(counts.cost, Some(most)).unwrap_or_default(),
            None,
            &say!(scope.text, deep_spend_aria, value = &value)?,
        ),
        value,
        ..Default::default()
    })
}

fn clicks(scope: &Scope, counts: &Counts) -> Fact {
    Fact::plain(
        scope.text.written.metric_clicks,
        scope.text.count(counts.clicks),
    )
}

fn cost_per_click(scope: &Scope, counts: &Counts) -> Fact {
    let w = &scope.text.written;
    match ratio(counts.cost, Some(counts.clicks)) {
        Some(cost) => Fact::plain(w.metric_cost_per_click, scope.money(cost)),
        None => Fact::missing(w.metric_cost_per_click, w.deep_no_clicks.into()),
    }
}

#[derive(Clone, Copy)]
enum Side {
    Above,
    Below,
    At,
}

fn side(value: f64, campaign: f64) -> Side {
    if value > campaign {
        Side::Above
    } else if value < campaign {
        Side::Below
    } else {
        Side::At
    }
}

#[derive(Clone, Copy)]
enum Against {
    Average,
    Rate,
}

fn compared(text: &Text, value: f64, campaign: f64, against: Against) -> Option<Badge> {
    let w = &text.written;
    let (kind, words) = match (side(value, campaign), against) {
        (Side::Above, Against::Average) => ("higher", w.deep_vs_average_above),
        (Side::Below, Against::Average) => ("lower", w.deep_vs_average_below),
        (Side::At, Against::Average) => ("equal", w.deep_vs_average_at),
        (Side::Above, Against::Rate) => ("higher", w.deep_vs_rate_above),
        (Side::Below, Against::Rate) => ("lower", w.deep_vs_rate_below),
        (Side::At, Against::Rate) => ("equal", w.deep_vs_rate_at),
    };
    Badge::new(kind, words)
}

fn most_spend(
    scope: &Scope,
    key: &str,
    template: &str,
    rows: &[&AuditRow],
    name: impl Fn(&AuditRow) -> String,
) -> Result<String> {
    let first = rows[0];
    fill(
        key,
        template,
        &[
            ("name", name(first)),
            ("spend", scope.money(first.counts.cost)),
            ("clicks", clicks_n(scope.text, first.counts.clicks)?),
        ],
    )
}

fn click_facts(scope: &Scope, counts: &Counts, most: f64) -> Result<Vec<Fact>> {
    Ok(vec![
        spend(scope, counts, most)?,
        clicks(scope, counts),
        cost_per_click(scope, counts),
    ])
}

fn click_columns(text: &Text) -> Vec<&'static str> {
    let w = &text.written;
    vec![w.metric_spend, w.metric_clicks, w.metric_cost_per_click]
}

fn empty(id: &'static str, title: &'static str, sentence: &'static str) -> Section {
    Section {
        id,
        title,
        sentence: sentence.into(),
        ..Default::default()
    }
}

fn search_terms(rows: &[AuditRow], scope: &Scope) -> Result<Section> {
    let text = scope.text;
    let w = &text.written;
    let (id, title) = ("search-terms", w.deep_terms_title);
    let rows = largest(rows);
    if rows.is_empty() {
        return Ok(empty(id, title, w.deep_empty_terms));
    }
    let most = rows[0].counts.cost;
    let wasted = |row: &AuditRow| row.counts.cost > 0.0 && row.counts.conversions <= 0.0;
    let sentence = if !scope.judged {
        most_spend(
            scope,
            "deep_most_spend_term",
            w.deep_most_spend_term,
            &rows,
            |row| row.name.clone(),
        )?
    } else {
        let without = rows.iter().filter(|row| wasted(row)).collect::<Vec<_>>();
        let cost = scope.money(without.iter().map(|row| row.counts.cost).sum());
        match (rows.len(), without.len()) {
            (1, 0) => w.deep_terms_one_converts.into(),
            (1, _) => say!(text, deep_terms_one_wasted, cost = cost)?,
            (all, 0) => say_plural!(text, deep_terms_all_convert, all as u64)?,
            (all, some) => {
                say_plural!(text, deep_terms_wasted, some as u64, all = all, cost = cost)?
            }
        }
    };
    let facts = |row: &AuditRow| -> Result<Vec<Fact>> {
        if !scope.judged {
            return click_facts(scope, &row.counts, most);
        }
        Ok(vec![
            spend(scope, &row.counts, most)?,
            clicks(scope, &row.counts),
            Fact {
                label: w.metric_conversions,
                value: text.count(row.counts.conversions),
                badge: wasted(row)
                    .then(|| Badge::new("flag", w.deep_flag_wasted))
                    .flatten(),
                ..Default::default()
            },
        ])
    };
    Ok(Section {
        id,
        title,
        name: w.deep_name_term,
        sentence,
        columns: if scope.judged {
            vec![w.metric_spend, w.metric_clicks, w.metric_conversions]
        } else {
            click_columns(text)
        },
        rows: rows
            .iter()
            .map(|row| {
                Ok(Row {
                    name: row.name.clone(),
                    tags: Vec::new(),
                    badge: None,
                    facts: facts(row)?,
                })
            })
            .collect::<Result<_>>()?,
        about: if scope.judged {
            vec![
                w.deep_terms_what,
                w.deep_measured,
                w.deep_terms_about_wasted,
            ]
        } else {
            vec![w.deep_terms_what, w.deep_measured, w.deep_cost_per_click_is]
        },
        actions: if scope.judged {
            vec![
                w.deep_terms_action_negative,
                w.deep_terms_action_add_converting,
                w.deep_terms_action_check_unconverting,
            ]
        } else {
            vec![w.deep_terms_action_negative, w.deep_terms_action_add]
        },
        limitation: if scope.judged {
            w.deep_terms_limit_judged
        } else {
            w.deep_terms_limit
        },
        ..Default::default()
    })
}

fn keywords(
    rows: &[AuditRow],
    match_types: &BTreeMap<&str, Vec<&'static str>>,
    scope: &Scope,
) -> Result<Section> {
    let text = scope.text;
    let w = &text.written;
    let (id, title) = ("keywords", w.deep_keywords_title);
    let rows = largest(rows);
    if rows.is_empty() {
        return Ok(empty(id, title, w.deep_empty_keywords));
    }
    let most = rows[0].counts.cost;
    let average = scope
        .campaign
        .filter(|_| scope.judged)
        .and_then(|campaign| ratio(campaign.cost, Some(campaign.conversions)));
    let cost_of = |row: &AuditRow| ratio(row.counts.cost, Some(row.counts.conversions));
    let without = rows.iter().filter(|row| cost_of(row).is_none()).count();
    let sentence = if !scope.judged {
        most_spend(
            scope,
            "deep_most_spend_keyword",
            w.deep_most_spend_keyword,
            &rows,
            |row| row.name.clone(),
        )?
    } else {
        match (rows.len(), average) {
            (1, _) if without == 1 => w.deep_keywords_one_none.into(),
            (1, Some(average)) => {
                let cost = cost_of(rows[0]).unwrap_or_default();
                let template = match side(cost, average) {
                    Side::Above => w.deep_keywords_one_above,
                    Side::Below => w.deep_keywords_one_below,
                    Side::At => w.deep_keywords_one_at,
                };
                fill(
                    "deep_keywords_one",
                    template,
                    &[
                        ("cost", scope.money(cost)),
                        ("average", scope.money(average)),
                    ],
                )?
            }
            (1, None) => w.deep_keywords_one_converts.into(),
            (all, Some(average)) => {
                let above = rows
                    .iter()
                    .filter(|row| cost_of(row).is_some_and(|cost| cost > average))
                    .count();
                let mut sentence = say_plural!(
                    text,
                    deep_keywords_above,
                    above as u64,
                    all = all,
                    average = scope.money(average)
                )?;
                if without > 0 {
                    sentence.push(' ');
                    sentence.push_str(&say_plural!(text, deep_keywords_without, without as u64)?);
                }
                sentence
            }
            (all, None) if without == 0 => {
                say_plural!(text, deep_keywords_all_convert, all as u64)?
            }
            (all, None) => say_plural!(text, deep_keywords_none, without as u64, all = all)?,
        }
    };
    let facts = |row: &AuditRow| -> Result<Vec<Fact>> {
        if !scope.judged {
            return click_facts(scope, &row.counts, most);
        }
        let label = w.metric_cost_per_conversion;
        Ok(vec![
            spend(scope, &row.counts, most)?,
            clicks(scope, &row.counts),
            match cost_of(row) {
                Some(cost) => Fact {
                    label,
                    value: scope.money(cost),
                    detail: say!(
                        text,
                        from_value,
                        value = conversions_n(text, row.counts.conversions)?
                    )?,
                    badge: average
                        .and_then(|average| compared(text, cost, average, Against::Average)),
                    ..Default::default()
                },
                None => Fact::missing(label, w.deep_no_conversions.into()),
            },
        ])
    };
    Ok(Section {
        id,
        title,
        name: w.deep_name_keyword,
        sentence,
        columns: if scope.judged {
            vec![
                w.metric_spend,
                w.metric_clicks,
                w.metric_cost_per_conversion,
            ]
        } else {
            click_columns(text)
        },
        rows: rows
            .iter()
            .map(|row| {
                Ok(Row {
                    name: row.name.clone(),
                    tags: match_types
                        .get(row.name.as_str())
                        .cloned()
                        .unwrap_or_default(),
                    badge: None,
                    facts: facts(row)?,
                })
            })
            .collect::<Result<_>>()?,
        about: if scope.judged {
            vec![
                w.deep_keywords_what,
                w.deep_keywords_about_cost,
                w.deep_keywords_about_verdict,
            ]
        } else {
            vec![w.deep_keywords_what, w.deep_cost_per_click_is]
        },
        terms: vec![
            (w.deep_match_exact, w.deep_term_exact),
            (w.deep_match_phrase, w.deep_term_phrase),
            (w.deep_match_broad, w.deep_term_broad),
        ],
        actions: if scope.judged {
            vec![
                w.deep_keywords_action_above,
                w.deep_keywords_action_wasted,
                w.deep_keywords_action_broad,
            ]
        } else {
            vec![
                w.deep_keywords_action_highest_cpc,
                w.deep_keywords_action_broad,
            ]
        },
        limitation: w.deep_keywords_limit,
        ..Default::default()
    })
}

fn page_name(row: &AuditRow) -> String {
    row.name
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .into()
}

fn landing_pages(rows: &[AuditRow], scope: &Scope) -> Result<Section> {
    let text = scope.text;
    let w = &text.written;
    let (id, title) = ("landing-pages", w.deep_pages_title);
    let rows = largest(rows);
    if rows.is_empty() {
        return Ok(empty(id, title, w.deep_empty_pages));
    }
    let most = rows[0].counts.cost;
    let campaign = scope
        .campaign
        .filter(|_| scope.judged)
        .and_then(|campaign| ratio(campaign.conversions, Some(campaign.clicks)));
    let rate_of = |row: &AuditRow| ratio(row.counts.conversions, Some(row.counts.clicks));
    let scale = rows
        .iter()
        .filter_map(|row| rate_of(row))
        .chain(campaign)
        .fold(0.0, f64::max);
    let sentence = match (rows.len(), campaign) {
        (1, Some(campaign)) if rate_of(rows[0]).is_some() => {
            let rate = rate_of(rows[0]).unwrap_or_default();
            let template = match side(rate, campaign) {
                Side::Above => w.deep_pages_one_above,
                Side::Below => w.deep_pages_one_below,
                Side::At => w.deep_pages_one_at,
            };
            fill(
                "deep_pages_one",
                template,
                &[
                    ("rate", percent(text, rate)),
                    ("campaign", percent(text, campaign)),
                ],
            )?
        }
        (all, Some(campaign)) if all > 1 => {
            let below = rows
                .iter()
                .filter(|row| rate_of(row).is_some_and(|rate| rate < campaign))
                .count();
            say_plural!(
                text,
                deep_pages_below,
                below as u64,
                all = all,
                campaign = percent(text, campaign)
            )?
        }
        _ => most_spend(
            scope,
            "deep_most_spend_page",
            w.deep_most_spend_page,
            &rows,
            page_name,
        )?,
    };
    let facts = |row: &AuditRow| -> Result<Vec<Fact>> {
        if !scope.judged {
            return click_facts(scope, &row.counts, most);
        }
        let label = w.deep_label_conversion_rate;
        Ok(vec![
            match rate_of(row) {
                Some(rate) => {
                    let value = percent(text, rate);
                    let aria = match campaign {
                        Some(campaign) => say!(
                            text,
                            deep_conversion_rate_against_aria,
                            value = &value,
                            campaign = percent(text, campaign)
                        )?,
                        None => say!(text, deep_conversion_rate_aria, value = &value)?,
                    };
                    Fact {
                        label,
                        detail: say!(
                            text,
                            deep_count_from,
                            part = conversions_n(text, row.counts.conversions)?,
                            total = clicks_n(text, row.counts.clicks)?
                        )?,
                        bar: bar(
                            ratio(rate, Some(scale)).unwrap_or_default(),
                            campaign.and_then(|campaign| ratio(campaign, Some(scale))),
                            &aria,
                        ),
                        badge: campaign
                            .and_then(|campaign| compared(text, rate, campaign, Against::Rate)),
                        value,
                        wide: false,
                    }
                }
                None => Fact::missing(label, w.deep_no_clicks.into()),
            },
            Fact::plain(w.metric_spend, scope.money(row.counts.cost)),
        ])
    };
    Ok(Section {
        id,
        title,
        name: w.deep_name_page,
        url: true,
        sentence,
        columns: if scope.judged {
            vec![w.deep_label_conversion_rate, w.metric_spend]
        } else {
            click_columns(text)
        },
        rows: rows
            .iter()
            .map(|row| {
                Ok(Row {
                    name: page_name(row),
                    tags: Vec::new(),
                    badge: None,
                    facts: facts(row)?,
                })
            })
            .collect::<Result<_>>()?,
        key: match campaign.filter(|_| scope.judged) {
            Some(campaign) => say!(text, deep_campaign_rate_key, rate = percent(text, campaign))?,
            None => String::new(),
        },
        about: if scope.judged {
            vec![
                w.deep_pages_what,
                w.deep_pages_about_rate,
                w.deep_pages_about_verdict,
            ]
        } else {
            vec![w.deep_pages_what, w.deep_cost_per_click_is]
        },
        actions: if scope.judged {
            vec![
                w.deep_pages_action_below,
                w.deep_pages_action_above,
                w.deep_pages_action_wrong,
            ]
        } else {
            vec![w.deep_pages_action_phone, w.deep_pages_action_wrong]
        },
        limitation: if scope.judged {
            w.deep_pages_limit_judged
        } else {
            w.deep_pages_limit
        },
        ..Default::default()
    })
}

fn ad_name(text: &Text, row: &AuditRow) -> String {
    let nameless = || text.written.deep_ad_without_name.to_owned();
    if row.id.is_some() {
        return if row.name == AD_WITHOUT_NAME {
            nameless()
        } else {
            row.name.clone()
        };
    }
    let name = row
        .name
        .rsplit_once(" (")
        .map_or(row.name.as_str(), |(name, _)| name)
        .trim();
    if name.is_empty() || name == STORED_UNNAMED_AD || name == AD_WITHOUT_NAME {
        nameless()
    } else {
        name.into()
    }
}

fn ad_strength(text: &Text, strength: &str) -> Option<Badge> {
    let w = &text.written;
    match strength {
        "POOR" => Badge::new("below", w.deep_strength_poor),
        "AVERAGE" => Badge::new("average", w.deep_strength_average),
        "GOOD" => Badge::new("above", w.deep_strength_good),
        "EXCELLENT" => Badge::new("above", w.deep_strength_excellent),
        _ => None,
    }
}

fn ads(rows: &[AuditRow], scope: &Scope) -> Result<Section> {
    let text = scope.text;
    let w = &text.written;
    let (id, title) = ("ads", w.deep_ads_title);
    let rows = largest(rows);
    if rows.is_empty() {
        return Ok(empty(id, title, w.deep_empty_ads));
    }
    let rate_of = |row: &AuditRow| ratio(row.counts.clicks, Some(row.counts.impressions));
    let seen = rows
        .iter()
        .filter_map(|row| Some((*row, rate_of(row)?)))
        .collect::<Vec<_>>();
    let scale = seen.iter().map(|(_, rate)| *rate).fold(0.0, f64::max);
    let sentence = match seen.as_slice() {
        [] => w.deep_ads_none.into(),
        [(row, rate)] => say!(
            text,
            deep_ads_one,
            rate = percent(text, *rate),
            clicks = clicks_n(text, row.counts.clicks)?,
            impressions = impressions_n(text, row.counts.impressions)?
        )?,
        seen => {
            let (row, rate) = seen
                .iter()
                .fold(seen[0], |best, ad| if ad.1 > best.1 { *ad } else { best });
            say_plural!(
                text,
                deep_ads_best,
                seen.len() as u64,
                name = ad_name(text, row),
                rate = percent(text, rate)
            )?
        }
    };
    let facts = |row: &AuditRow| -> Result<Vec<Fact>> {
        let label = w.deep_label_click_rate;
        let mut facts = vec![
            match rate_of(row) {
                Some(rate) => {
                    let value = percent(text, rate);
                    Fact {
                        label,
                        detail: say!(
                            text,
                            deep_count_from,
                            part = clicks_n(text, row.counts.clicks)?,
                            total = impressions_n(text, row.counts.impressions)?
                        )?,
                        bar: bar(
                            ratio(rate, Some(scale)).unwrap_or_default(),
                            None,
                            &say!(text, deep_click_rate_aria, value = &value)?,
                        ),
                        value,
                        ..Default::default()
                    }
                }
                None => Fact::missing(label, w.deep_no_impressions.into()),
            },
            Fact::plain(w.metric_spend, scope.money(row.counts.cost)),
        ];
        if scope.judged {
            facts.push(Fact::plain(
                w.metric_conversions,
                text.count(row.counts.conversions),
            ));
        }
        Ok(facts)
    };
    Ok(Section {
        id,
        title,
        name: w.deep_name_ad,
        sentence,
        columns: if scope.judged {
            vec![
                w.deep_label_click_rate,
                w.metric_spend,
                w.metric_conversions,
            ]
        } else {
            vec![w.deep_label_click_rate, w.metric_spend]
        },
        rows: rows
            .iter()
            .map(|row| {
                Ok(Row {
                    name: ad_name(text, row),
                    tags: Vec::new(),
                    badge: ad_strength(text, &row.strength),
                    facts: facts(row)?,
                })
            })
            .collect::<Result<_>>()?,
        about: vec![w.deep_ads_about_click_rate, w.deep_ads_about_strength],
        actions: vec![
            w.deep_ads_action_strength,
            w.deep_ads_action_lowest,
            w.deep_ads_action_none,
        ],
        limitation: w.deep_ads_limit,
        ..Default::default()
    })
}

fn scope<'a>(
    text: &'a Text,
    audit: &'a Audit,
    campaign_id: i64,
    currency: &'a str,
    judged: bool,
) -> Scope<'a> {
    Scope {
        text,
        currency,
        judged,
        campaign: audit
            .campaigns
            .iter()
            .find(|row| row.id == Some(campaign_id))
            .map(|row| &row.counts),
    }
}

pub fn search_terms_section(
    text: &Text,
    audit: &Audit,
    campaign_id: i64,
    currency: &str,
    judged: bool,
) -> Result<Section> {
    search_terms(
        &audit.search_terms,
        &scope(text, audit, campaign_id, currency, judged),
    )
}

pub fn sections(
    text: &Text,
    quality: Section,
    audit: Option<&Audit>,
    campaign_id: i64,
    currency: &str,
    judged: bool,
) -> Result<Vec<Section>> {
    let Some(audit) = audit.filter(|audit| {
        !(audit.search_terms.is_empty()
            && audit.keywords.is_empty()
            && audit.landing_pages.is_empty()
            && audit.ads.is_empty())
    }) else {
        return Ok(vec![quality]);
    };
    let scope = scope(text, audit, campaign_id, currency, judged);
    let mut match_types: BTreeMap<&str, Vec<&'static str>> = BTreeMap::new();
    for row in &quality.rows {
        let known = match_types.entry(row.name.as_str()).or_default();
        for tag in &row.tags {
            if !known.contains(tag) {
                known.push(tag);
            }
        }
    }
    let rest = [
        search_terms(&audit.search_terms, &scope)?,
        keywords(&audit.keywords, &match_types, &scope)?,
        landing_pages(&audit.landing_pages, &scope)?,
        ads(&audit.ads, &scope)?,
    ];
    Ok(std::iter::once(quality).chain(rest).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{language::english, workspace::monitoring::KeywordQuality};

    fn row(name: &str, cost: f64, clicks: f64, impressions: f64, conversions: f64) -> AuditRow {
        AuditRow {
            name: name.into(),
            counts: Counts {
                impressions,
                clicks,
                conversions,
                cost,
                value: 0.0,
            },
            ..Default::default()
        }
    }

    fn campaign() -> AuditRow {
        AuditRow {
            id: Some(701),
            ..row("Search - Emergency", 4000.0, 2000.0, 40000.0, 100.0)
        }
    }

    fn score(
        keyword: &str,
        kind: &str,
        score: Option<i64>,
        before: Option<Option<i64>>,
    ) -> KeywordScore {
        KeywordScore {
            quality: KeywordQuality {
                campaign_id: 701,
                ad_group_id: 55,
                criterion_id: 1,
                keyword: keyword.into(),
                match_type: kind.into(),
                cost: 10.0,
                score,
                ad_relevance: score.map(|_| "AVERAGE".into()),
                landing_page: score.map(|_| "ABOVE_AVERAGE".into()),
                expected_ctr: score.map(|_| "BELOW_AVERAGE".into()),
            },
            previous: before.flatten(),
            known_before: before.is_some(),
        }
    }

    fn of(audit: &Audit, judged: bool) -> Vec<Section> {
        sections(
            english(),
            quality(
                english(),
                &[score("emergency dentist", "EXACT", Some(5), None)],
                false,
            )
            .unwrap(),
            Some(audit),
            701,
            "USD",
            judged,
        )
        .unwrap()
    }

    fn words(section: &Section) -> String {
        serde_json::to_string(section).unwrap().to_lowercase()
    }

    #[test]
    fn a_quality_score_has_a_bar_a_change_and_the_three_levels_of_google() {
        let section = quality(
            english(),
            &[
                score("emergency dentist", "EXACT", Some(5), Some(Some(7))),
                score("dentist open now", "PHRASE", Some(3), None),
                score("24 hour dentist", "BROAD", Some(8), Some(Some(8))),
                score("urgent dentist", "EXACT", Some(9), Some(Some(6))),
                score("dental clinic", "EXACT", Some(6), Some(None)),
            ],
            true,
        )
        .unwrap();
        assert_eq!(
            section.note,
            "Each change is against the Deep check before."
        );
        let first = &section.rows[0];
        assert_eq!(first.tags, ["Exact match"]);
        assert_eq!(first.facts[0].value, "5 of 10");
        assert_eq!(first.facts[0].bar.as_ref().unwrap().drawn, Drawn::Score(5));
        assert_eq!(first.facts[0].badge, Badge::new("lower", "Down 2"));
        assert_eq!(
            first.facts[1..]
                .iter()
                .map(|fact| (fact.label, fact.badge.clone().unwrap()))
                .collect::<Vec<_>>(),
            [
                (
                    "Expected clickthrough rate",
                    Badge::new("below", "Below average").unwrap()
                ),
                ("Ad relevance", Badge::new("average", "Average").unwrap()),
                (
                    "Landing page experience",
                    Badge::new("above", "Above average").unwrap()
                ),
            ]
        );
        assert_eq!(section.rows[1].facts[0].detail, "New keyword");
        assert_eq!(section.rows[1].facts[0].badge, None);
        assert_eq!(
            section.rows[2].facts[0].badge,
            Badge::new("equal", "No change")
        );
        assert_eq!(section.rows[3].facts[0].badge, Badge::new("higher", "Up 3"));
        assert_eq!(section.rows[4].facts[0].detail, "No score before");
    }

    #[test]
    fn a_keyword_without_a_score_has_one_explanation_and_the_first_deep_check_one_note() {
        let section = quality(
            english(),
            &[
                score("emergency dentist", "EXACT", Some(5), None),
                score("tooth pain", "BROAD", None, None),
            ],
            false,
        )
        .unwrap();
        assert_eq!(
            section.note,
            "This is the first Deep check with Quality scores. The change of each score appears after the next Deep check."
        );
        assert_eq!(section.rows[0].facts[0].badge, None);
        assert_eq!(section.rows[0].facts[0].detail, "");
        assert_eq!(
            section.rows[1].facts,
            [Fact {
                label: "Quality score",
                detail: "Google Ads gives no Quality score yet. It needs more information about this keyword. You do not need to do anything.".into(),
                wide: true,
                ..Default::default()
            }]
        );
        let text = words(&section);
        assert!(!text.contains("not available"));
        assert!(!text.contains("to compare with"));
        assert_eq!(text.matches("first deep check").count(), 1);
        assert!(!text.contains("conversion"));

        let none = quality(
            english(),
            &[score("tooth pain", "BROAD", None, None)],
            false,
        )
        .unwrap();
        assert_eq!(none.note, "");
    }

    #[test]
    fn a_search_term_with_spend_and_no_conversions_has_a_flag_only_when_judged() {
        let audit = Audit {
            search_terms: vec![
                row("emergency dentist", 400.0, 200.0, 2000.0, 12.0),
                row("free dental care", 96.0, 61.0, 900.0, 0.0),
                row("dentist jobs", 24.0, 10.0, 300.0, 0.0),
                row("dentist", 0.0, 0.0, 40.0, 0.0),
            ],
            ..Default::default()
        };
        let judged = &of(&audit, true)[1];
        assert_eq!(
            judged.sentence,
            "2 of the 4 search terms with the most spend have spend and no conversions: $120 together."
        );
        assert_eq!(judged.columns, ["Spend", "Clicks", "Conversions"]);
        assert_eq!(
            judged
                .rows
                .iter()
                .map(|row| row.facts[2].badge.clone())
                .collect::<Vec<_>>(),
            [
                None,
                Badge::new("flag", "Spend, no conversions"),
                Badge::new("flag", "Spend, no conversions"),
                None
            ]
        );
        assert_eq!(judged.rows[1].facts[0].value, "$96.00");
        assert_eq!(
            judged.rows[1].facts[0].bar.as_ref().unwrap().drawn,
            Drawn::Share {
                share: 0.24,
                mark: None
            }
        );

        let unjudged = &of(&audit, false)[1];
        assert_eq!(
            unjudged.sentence,
            "The search term with the most spend is emergency dentist: $400 for 200 clicks."
        );
        assert_eq!(unjudged.columns, ["Spend", "Clicks", "Cost per click"]);
        assert_eq!(unjudged.rows[1].facts[2].value, "$1.57");
        assert_eq!(
            unjudged.rows[3].facts[2].detail,
            "No clicks in these 30 days"
        );
        for section in of(&audit, false) {
            let text = words(&section);
            assert!(!text.contains("conversion"), "{text}");
            assert!(!text.contains("flag"), "{text}");
        }
    }

    #[test]
    fn a_keyword_is_compared_with_the_campaign_average_and_has_its_match_type() {
        let audit = Audit {
            campaigns: vec![campaign()],
            keywords: vec![
                row("emergency dentist", 900.0, 300.0, 3000.0, 30.0),
                row("dentist open now", 500.0, 200.0, 2000.0, 10.0),
                row("tooth pain", 80.0, 40.0, 900.0, 0.0),
            ],
            ..Default::default()
        };
        let section = &of(&audit, true)[2];
        assert_eq!(
            section.sentence,
            "1 of the 3 keywords with the most spend has a cost per conversion above the Campaign average of $40.00. 1 has no conversions."
        );
        assert_eq!(section.rows[0].tags, ["Exact match"]);
        assert!(section.rows[1].tags.is_empty());
        let cost = |index: usize| &section.rows[index].facts[2];
        assert_eq!(cost(0).value, "$30.00");
        assert_eq!(cost(0).detail, "from 30 conversions");
        assert_eq!(
            cost(0).badge,
            Badge::new("lower", "Below the Campaign average")
        );
        assert_eq!(
            cost(1).badge,
            Badge::new("higher", "Above the Campaign average")
        );
        assert_eq!(cost(2).value, "");
        assert_eq!(cost(2).detail, "No conversions in these 30 days");
        assert_eq!(cost(2).badge, None);

        let alone = Audit {
            campaigns: Vec::new(),
            ..audit
        };
        let section = &of(&alone, true)[2];
        assert_eq!(
            section.sentence,
            "1 of the 3 keywords with the most spend has no conversions."
        );
        assert_eq!(section.rows[0].facts[2].badge, None);
    }

    #[test]
    fn a_landing_page_has_its_conversion_rate_against_the_campaign_rate() {
        let audit = Audit {
            campaigns: vec![campaign()],
            landing_pages: vec![
                row(
                    "https://harbourdental.example/emergency/",
                    600.0,
                    300.0,
                    3000.0,
                    30.0,
                ),
                row("http://harbourdental.example", 300.0, 200.0, 2000.0, 4.0),
                row(
                    "https://harbourdental.example/prices",
                    20.0,
                    0.0,
                    100.0,
                    0.0,
                ),
            ],
            ..Default::default()
        };
        let section = &of(&audit, true)[3];
        assert!(section.url);
        assert_eq!(
            section.sentence,
            "1 of the 3 landing pages with the most spend has a conversion rate below the Campaign rate of 5.0%."
        );
        assert_eq!(section.key, "Campaign rate: 5.0%");
        let rate = |index: usize| &section.rows[index].facts[0];
        assert_eq!(section.rows[0].name, "harbourdental.example/emergency");
        assert_eq!(rate(0).value, "10.0%");
        assert_eq!(rate(0).detail, "30 conversions from 300 clicks");
        assert_eq!(
            rate(0).badge,
            Badge::new("higher", "Above the Campaign rate")
        );
        assert_eq!(
            rate(0).bar.as_ref().unwrap().drawn,
            Drawn::Share {
                share: 1.0,
                mark: Some(0.5)
            }
        );
        assert!(rate(0)
            .bar
            .as_ref()
            .unwrap()
            .svg
            .contains(r#"aria-label="Conversion rate 10.0%. Campaign rate 5.0%""#));
        assert_eq!(
            rate(1).badge,
            Badge::new("lower", "Below the Campaign rate")
        );
        assert_eq!(rate(2).detail, "No clicks in these 30 days");
        assert_eq!(rate(2).bar, None);
        assert!(section
            .limitation
            .contains("KeySend does not read your website analytics: bounce rate and time on page are not available."));

        let unjudged = &of(&audit, false)[3];
        assert_eq!(unjudged.key, "");
        assert_eq!(
            unjudged.sentence,
            "The landing page with the most spend is harbourdental.example/emergency: $600 for 300 clicks."
        );
    }

    #[test]
    fn an_ad_has_its_name_its_ad_strength_and_one_empty_state_for_each_cause() {
        let ad =
            |name: &str, id: Option<i64>, clicks: f64, impressions: f64, strength: &str| AuditRow {
                id,
                strength: strength.into(),
                ..row(name, clicks, clicks, impressions, 0.0)
            };
        let audit = Audit {
            ads: vec![
                ad(
                    "Emergency Dentist | Open Today (24/7)",
                    Some(9001),
                    134.0,
                    720.0,
                    "GOOD",
                ),
                ad("Not available (9002)", None, 90.0, 900.0, "POOR"),
                ad(" (9003)", None, 80.0, 800.0, ""),
                ad("Autumn banner (9004)", None, 70.0, 700.0, "Not available"),
                ad(
                    "Responsive display ad in ad group Emergency",
                    Some(9005),
                    0.0,
                    0.0,
                    "EXCELLENT",
                ),
            ],
            ..Default::default()
        };
        let section = &of(&audit, false)[4];
        assert_eq!(
            section.sentence,
            "Emergency Dentist | Open Today (24/7) has the highest click rate of the 4 ads with impressions: 18.6%."
        );
        assert_eq!(section.columns, ["Click rate", "Spend"]);
        assert_eq!(
            section
                .rows
                .iter()
                .map(|row| (
                    row.name.as_str(),
                    row.facts[0].value.as_str(),
                    row.facts[0].detail.as_str(),
                    row.badge
                        .as_ref()
                        .map(|badge| (badge.kind, badge.text.as_str()))
                ))
                .collect::<Vec<_>>(),
            [
                (
                    "Emergency Dentist | Open Today (24/7)",
                    "18.6%",
                    "134 clicks from 720 impressions",
                    Some(("above", "Ad strength: Good"))
                ),
                (
                    "Ad without a name",
                    "10.0%",
                    "90 clicks from 900 impressions",
                    Some(("below", "Ad strength: Poor"))
                ),
                (
                    "Ad without a name",
                    "10.0%",
                    "80 clicks from 800 impressions",
                    None
                ),
                (
                    "Autumn banner",
                    "10.0%",
                    "70 clicks from 700 impressions",
                    None
                ),
                (
                    "Responsive display ad in ad group Emergency",
                    "",
                    "No impressions in these 30 days",
                    Some(("above", "Ad strength: Excellent"))
                ),
            ]
        );
        assert!(!words(section).contains("not available"));
        assert_eq!(
            of(&audit, true)[4].columns,
            ["Click rate", "Spend", "Conversions"]
        );

        let unseen = Audit {
            ads: vec![ad("Autumn banner", Some(1), 0.0, 0.0, "")],
            ..Default::default()
        };
        assert_eq!(
            of(&unseen, true)[4].sentence,
            "No ad had impressions in these 30 days."
        );
    }

    #[test]
    fn a_kind_without_rows_is_one_sentence_and_a_campaign_without_rows_has_the_scores_only() {
        let audit = Audit {
            keywords: vec![row("emergency dentist", 900.0, 300.0, 3000.0, 30.0)],
            ..Default::default()
        };
        let all = of(&audit, true);
        assert_eq!(
            all.iter().map(|section| section.id).collect::<Vec<_>>(),
            [
                "keyword-quality",
                "search-terms",
                "keywords",
                "landing-pages",
                "ads"
            ]
        );
        assert_eq!(
            all[1].sentence,
            "The Deep check found no search term with numbers for this Campaign in these 30 days. Not every type of Campaign has search terms. You do not need to do anything."
        );
        assert!(all[1].rows.is_empty() && all[1].actions.is_empty());
        assert_eq!(
            all[2].sentence,
            "The keyword with the most spend has conversions."
        );
        assert_eq!(of(&Audit::default(), true).len(), 1);
        assert_eq!(
            sections(
                english(),
                quality_missing(english(), "None.".into()),
                None,
                701,
                "USD",
                true
            )
            .unwrap()
            .len(),
            1
        );
    }
}
