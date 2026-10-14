use std::fmt::Write as _;

use loco_rs::Result;

use crate::language::{fill, say, say_plural, Text, Written};

use super::{
    chars, esc, share_text, tip, width_class, Chart, Key, Sentence, ShareRow, Table, CHAR, NARROW,
};

const STACK_WIDE: f64 = 430.0;
const LABEL_SHARE: f64 = 5.0;
const NAMED_LOSS: f64 = 0.5;

pub struct SearchShare {
    pub name: String,
    pub href: Option<String>,
    pub shares: Option<(f64, f64, f64)>,
    pub limit_days: i64,
    pub days: i64,
}

fn parts(written: &Written) -> [(&'static str, &'static str, &'static str); 3] {
    [
        ("shown", written.ss_label_shown, written.ss_title_shown),
        ("budget", written.ss_label_budget, written.ss_title_budget),
        ("rank", written.ss_label_rank, written.ss_title_rank),
    ]
}

fn left_out(text: &Text, row: &SearchShare) -> Result<String> {
    if row.limit_days == 0 {
        Ok(String::new())
    } else {
        Ok(format!(
            " {}",
            say_plural!(
                text,
                ss_left_out,
                u64::try_from(row.limit_days).unwrap_or_default(),
                limit = row.limit_days,
                days = row.days
            )?
        ))
    }
}

fn bar(text: &Text, row: &SearchShare, shares: (f64, f64, f64)) -> Result<String> {
    let percents = [shares.0 * 100.0, shares.1 * 100.0, shares.2 * 100.0];
    let total = percents.iter().sum::<f64>();
    let mut svg = format!(
        r#"<svg class="chart__bars" height="28" role="group" aria-label="{}">"#,
        esc(&row.name)
    );
    let mut x = 0.0;
    for ((kind, label, title), percent) in parts(&text.written).into_iter().zip(percents) {
        let width = if total > 0.0 {
            percent / total * 100.0
        } else {
            0.0
        };
        if width <= 0.0 {
            continue;
        }
        let value = share_text(text, percent);
        let middle = x + width / 2.0;
        let _ = write!(
            svg,
            r#"<g class="pt" tabindex="0" role="img"><title>{}</title><rect class="seg seg--{kind}" x="{x:.2}%" y="0" width="{width:.2}%" height="28" rx="4"/>"#,
            esc(&fill(
                "ss_title",
                title,
                &[("name", row.name.as_str()), ("value", value.as_str())]
            )?),
        );
        let fits =
            |plot: f64| percent > LABEL_SHARE && width / 100.0 * plot >= chars(&value) * CHAR + 8.0;
        if let Some(class) = width_class(fits(STACK_WIDE), fits(NARROW)) {
            let _ = write!(
                svg,
                r#"<text class="seg__label seg__label--{kind}{class}" x="{middle:.2}%" y="18" text-anchor="middle">{}</text>"#,
                esc(&value)
            );
        }
        let _ = write!(
            svg,
            r#"<svg x="{middle:.2}%" width="1" height="1" overflow="visible">{}</svg></g>"#,
            tip(
                0.0,
                -30.0,
                &[say!(text, ss_tip, label = label, value = &value)?],
                middle > 50.0
            )
        );
        x += width;
    }
    svg.push_str("</svg>");
    Ok(svg)
}

pub fn search_share(text: &Text, title: &str, rows: Vec<SearchShare>) -> Result<Chart> {
    let written = &text.written;
    if rows.is_empty() {
        return Ok(Chart::empty(
            title.into(),
            written.missing_search_share_none,
        ));
    }
    let exact = rows
        .iter()
        .filter_map(|row| row.shares.map(|shares| (row, shares)))
        .collect::<Vec<_>>();
    let named = exact
        .iter()
        .max_by(|a, b| a.1 .1.total_cmp(&b.1 .1))
        .filter(|(_, shares)| shares.1 * 100.0 >= NAMED_LOSS);
    let alone = if rows.len() == 1 {
        exact.as_slice()
    } else {
        &[]
    };
    let sentence = match (named, alone) {
        (Some((row, shares)), _) => Sentence {
            strong: row.name.clone(),
            text: say!(text, ss_named, share = share_text(text, shares.1 * 100.0))?,
            mark: String::new(),
            after: left_out(text, row)?,
        },
        (None, [(row, shares)]) => Sentence {
            strong: row.name.clone(),
            text: say!(
                text,
                ss_alone,
                shown = share_text(text, shares.0 * 100.0),
                budget = share_text(text, shares.1 * 100.0)
            )?,
            mark: String::new(),
            after: left_out(text, row)?,
        },
        (None, _) if exact.is_empty() => {
            Sentence::plain(if rows.iter().all(|row| row.limit_days == row.days) {
                written.ss_all_limited
            } else if rows.iter().all(|row| row.limit_days == 0) {
                written.ss_all_zero
            } else {
                written.ss_none_exact
            })
        }
        (None, _) => Sentence::plain(written.ss_under_one),
    };
    let table = Table {
        caption: title.into(),
        headers: vec![
            written.shares_campaign_noun.into(),
            written.ss_label_shown.into(),
            written.ss_label_budget.into(),
            written.ss_label_rank.into(),
            written.ss_header_days.into(),
        ],
        rows: rows
            .iter()
            .map(|row| {
                let cell = |share: Option<f64>| {
                    share.map_or_else(
                        || written.ss_under_ten.into(),
                        |share| share_text(text, share * 100.0),
                    )
                };
                Ok(vec![
                    row.name.clone(),
                    cell(row.shares.map(|shares| shares.0)),
                    row.shares.map_or_else(
                        || written.not_available.into(),
                        |shares| share_text(text, shares.1 * 100.0),
                    ),
                    row.shares.map_or_else(
                        || written.not_available.into(),
                        |shares| share_text(text, shares.2 * 100.0),
                    ),
                    say!(text, ss_days_of, limit = row.limit_days, days = row.days)?,
                ])
            })
            .collect::<Result<_>>()?,
        ..Default::default()
    };
    let bars = rows
        .iter()
        .map(|row| {
            Ok(ShareRow {
                name: row.name.clone(),
                href: row.href.clone(),
                bars: None,
                values: row
                    .shares
                    .map(|shares| {
                        parts(written)
                            .iter()
                            .zip([shares.0, shares.1, shares.2])
                            .map(|((_, label, _), share)| {
                                say!(
                                    text,
                                    ss_value,
                                    label = label,
                                    value = share_text(text, share * 100.0)
                                )
                            })
                            .collect::<Result<Vec<_>>>()
                            .map(|values| values.join(" · "))
                    })
                    .transpose()?,
                svg: match row.shares {
                    Some(shares) => bar(text, row, shares)?,
                    None => format!(
                        r#"<svg class="chart__bars" height="28" role="img" aria-label="{}"><text x="0" y="18">{}</text></svg>"#,
                        esc(&say!(text, ss_aria_unknown, name = &row.name)?),
                        esc(written.ss_unknown_text)
                    ),
                },
            })
        })
        .collect::<Result<_>>()?;
    Ok(Chart {
        title: title.into(),
        sentence,
        svg: None,
        rows: bars,
        keys: parts(written)
            .iter()
            .map(|(kind, label, _)| Key {
                kind,
                label: (*label).into(),
            })
            .collect(),
        table: Some(table),
    })
}

#[cfg(test)]
mod tests {
    use super::super::english_calls::search_share;
    use super::*;

    fn row(name: &str, shares: Option<(f64, f64, f64)>, limit_days: i64) -> SearchShare {
        SearchShare {
            name: name.into(),
            href: Some(format!("/c/{name}")),
            shares,
            limit_days,
            days: 7,
        }
    }

    #[test]
    fn each_campaign_has_one_stacked_bar_and_the_sentence_names_the_largest_budget_loss() {
        let chart = search_share(
            "Searches that the ads did not appear on, last 7 days",
            vec![
                row("Search - Emergency", Some((0.52, 0.31, 0.17)), 0),
                row("Search - Implants", Some((0.64, 0.06, 0.30)), 2),
                row("Search - Brand", Some((0.91, 0.02, 0.07)), 0),
            ],
        );
        assert_eq!(
            chart.sentence.words(),
            "Search - Emergency did not appear on 31% of eligible searches. Google Ads reports budget as the limit for these."
        );
        assert_eq!(chart.sentence.strong, "Search - Emergency");
        let first = &chart.rows[0].svg;
        assert!(first.starts_with(
            r#"<svg class="chart__bars" height="28" role="group" aria-label="Search - Emergency"><g class="pt" tabindex="0" role="img"><title>Search - Emergency: shown on 52% of eligible searches</title><rect class="seg seg--shown" x="0.00%" y="0" width="52.00%" height="28" rx="4"/><text class="seg__label seg__label--shown" x="26.00%" y="18" text-anchor="middle">52%</text>"#
        ), "{first}");
        assert!(first.contains(
            r#"<rect class="seg seg--budget" x="52.00%" y="0" width="31.00%" height="28" rx="4"/><text class="seg__label seg__label--budget" x="67.50%" y="18" text-anchor="middle">31%</text>"#
        ));
        assert!(first.contains(
            r#"<rect class="seg seg--rank" x="83.00%" y="0" width="17.00%" height="28" rx="4"/><text class="seg__label seg__label--rank" x="91.50%" y="18" text-anchor="middle">17%</text>"#
        ));
        assert!(first.contains(">Missed: budget: 31% of eligible searches</text>"));
        assert_eq!(first.matches(r#"class="pt" tabindex="0""#).count(), 3);

        let brand = &chart.rows[2].svg;
        assert!(brand.contains(r#"<rect class="seg seg--budget" x="91.00%" y="0" width="2.00%""#));
        assert!(!brand.contains(">2%</text>"));
        assert!(brand.contains(
            r#"<text class="seg__label seg__label--rank wide" x="96.50%" y="18" text-anchor="middle">7%</text>"#
        ), "{brand}");

        let table = chart.table.unwrap();
        assert_eq!(
            table.rows[1],
            ["Search - Implants", "64%", "6%", "30%", "2 of 7"]
        );
        assert_eq!(
            chart
                .keys
                .iter()
                .map(|key| (key.kind, key.label.as_str()))
                .collect::<Vec<_>>(),
            [
                ("shown", "Shown"),
                ("budget", "Missed: budget"),
                ("rank", "Missed: ad rank")
            ]
        );
    }

    #[test]
    fn under_ten_percent_is_never_a_number() {
        let chart = search_share(
            "Searches",
            vec![
                row("Search - New", None, 7),
                row("Search - Brand", Some((0.8, 0.0, 0.2)), 1),
            ],
        );
        assert_eq!(
            chart.sentence.words(),
            "Google Ads reports budget as the limit for under 1% of eligible searches in these days."
        );
        assert_eq!(
            chart.rows[0].svg,
            r#"<svg class="chart__bars" height="28" role="img" aria-label="Search - New: under 10% shown, no exact share"><text x="0" y="18">Under 10% shown: no exact share</text></svg>"#
        );
        assert_eq!(
            chart.table.unwrap().rows[0],
            [
                "Search - New",
                "Under 10%",
                "Not available",
                "Not available",
                "7 of 7"
            ]
        );

        let alone = search_share(
            "Searches",
            vec![row("Search - Brand", Some((0.8, 0.0, 0.2)), 1)],
        );
        assert_eq!(
            alone.sentence.words(),
            "Search - Brand appeared on 80% of eligible searches. Google Ads reports budget as the limit for 0% of them. 1 of its 7 days is left out: Google Ads reports under 10% and gives no exact share."
        );

        let none = search_share("Searches", vec![row("Search - New", None, 7)]);
        assert_eq!(
            none.sentence.words(),
            "Google Ads reports under 10% for each of these days and gives no exact share."
        );

        let empty = search_share("Searches", vec![]);
        assert_eq!(
            empty.sentence.words(),
            "Google Ads has no search impression share for these days."
        );
        assert!(empty.table.is_none());
    }

    #[test]
    fn days_without_a_share_do_not_say_under_ten_percent() {
        let said = |rows| search_share("Searches", rows).sentence.words();
        assert_eq!(
            said(vec![row("Search - New", None, 7)]),
            "Google Ads reports under 10% for each of these days and gives no exact share."
        );
        assert_eq!(
            said(vec![row("Search - New", None, 0)]),
            "Google Ads reports no search impression share for these days."
        );
        assert_eq!(
            said(vec![
                row("Search - New", None, 0),
                row("Search - Old", None, 0)
            ]),
            "Google Ads reports no search impression share for these days."
        );
        assert_eq!(
            said(vec![row("Search - New", None, 3)]),
            "Google Ads reports no exact search impression share for these days."
        );
        assert_eq!(
            said(vec![
                row("Search - New", None, 7),
                row("Search - Old", None, 0)
            ]),
            "Google Ads reports no exact search impression share for these days."
        );
    }
}
