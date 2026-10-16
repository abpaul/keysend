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
    pub exact_days: i64,
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

#[derive(Clone, Copy)]
enum Loss {
    Budget,
    Rank,
}

fn largest<'a>(
    exact: &[(&'a SearchShare, (f64, f64, f64))],
) -> Option<(&'a SearchShare, Loss, f64)> {
    exact
        .iter()
        .flat_map(|(row, shares)| [(*row, Loss::Budget, shares.1), (*row, Loss::Rank, shares.2)])
        .max_by(|a, b| a.2.total_cmp(&b.2))
        .filter(|(_, _, share)| share * 100.0 >= NAMED_LOSS)
}

fn coverage(text: &Text, row: &SearchShare) -> Result<String> {
    if row.exact_days >= row.days {
        return Ok(String::new());
    }
    let mut said = format!(
        " {}",
        say_plural!(
            text,
            ss_coverage,
            u64::try_from(row.days).unwrap_or_default(),
            exact = row.exact_days
        )?
    );
    if row.limit_days > 0 {
        let _ = write!(
            said,
            " {}",
            say_plural!(
                text,
                ss_lower,
                u64::try_from(row.limit_days).unwrap_or_default(),
                days = row.days
            )?
        );
    }
    Ok(said)
}

fn bar(text: &Text, row: &SearchShare, shares: (f64, f64, f64)) -> Result<String> {
    let percents = [shares.0 * 100.0, shares.1 * 100.0, shares.2 * 100.0];
    let mut svg = format!(
        r#"<svg class="chart__bars" height="28" role="group" aria-label="{}">"#,
        esc(&row.name)
    );
    let mut x = 0.0;
    for ((kind, label, title), percent) in parts(&text.written).into_iter().zip(percents) {
        let width = percent.min(100.0 - x);
        if width <= 0.0 {
            continue;
        }
        let value = share_text(text, percent);
        let middle = x + width / 2.0;
        let _ = write!(
            svg,
            r#"<g class="pt" tabindex="0" role="img" aria-label="{}"><rect class="seg seg--{kind}" x="{x:.2}%" y="0" width="{width:.2}%" height="28" rx="4"/>"#,
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
    let named = largest(&exact);
    let alone = if rows.len() == 1 {
        exact.as_slice()
    } else {
        &[]
    };
    let sentence = match (named, alone) {
        (Some((row, loss, share)), _) => Sentence {
            strong: row.name.clone(),
            text: match loss {
                Loss::Budget => say!(
                    text,
                    ss_named_budget,
                    share = share_text(text, share * 100.0)
                )?,
                Loss::Rank => say!(text, ss_named_rank, share = share_text(text, share * 100.0))?,
            },
            mark: String::new(),
            after: coverage(text, row)?,
        },
        (None, [(row, shares)]) => Sentence {
            strong: row.name.clone(),
            text: say!(text, ss_alone, shown = share_text(text, shares.0 * 100.0))?,
            mark: String::new(),
            after: coverage(text, row)?,
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
                let shown = row.shares.map_or_else(
                    || {
                        if row.days > 0 && row.limit_days == row.days {
                            written.ss_under_ten.into()
                        } else {
                            written.not_available.into()
                        }
                    },
                    |shares| share_text(text, shares.0 * 100.0),
                );
                Ok(vec![
                    row.name.clone(),
                    shown,
                    row.shares.map_or_else(
                        || written.not_available.into(),
                        |shares| share_text(text, shares.1 * 100.0),
                    ),
                    row.shares.map_or_else(
                        || written.not_available.into(),
                        |shares| share_text(text, shares.2 * 100.0),
                    ),
                    say!(text, ss_days_of, exact = row.exact_days, days = row.days)?,
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

    fn row(
        name: &str,
        shares: Option<(f64, f64, f64)>,
        exact_days: i64,
        limit_days: i64,
    ) -> SearchShare {
        SearchShare {
            name: name.into(),
            href: Some(format!("/c/{name}")),
            shares,
            exact_days,
            limit_days,
            days: 7,
        }
    }

    #[test]
    fn each_campaign_has_one_stacked_bar_and_the_sentence_names_the_largest_loss() {
        let chart = search_share(
            "Search impression share, last 7 days",
            vec![
                row("Search - Emergency", Some((0.52, 0.31, 0.17)), 7, 0),
                row("Search - Implants", Some((0.64, 0.06, 0.30)), 5, 2),
                row("Search - Brand", Some((0.91, 0.02, 0.07)), 7, 0),
            ],
        );
        assert_eq!(
            chart.sentence.words(),
            "Search - Emergency lost 31% of its estimated eligible Search impressions to budget. That is the largest loss in this chart."
        );
        assert_eq!(chart.sentence.strong, "Search - Emergency");
        let first = &chart.rows[0].svg;
        assert!(first.starts_with(
            r#"<svg class="chart__bars" height="28" role="group" aria-label="Search - Emergency"><g class="pt" tabindex="0" role="img" aria-label="Search - Emergency: shown for 52% of the estimated eligible Search impressions"><rect class="seg seg--shown" x="0.00%" y="0" width="52.00%" height="28" rx="4"/><text class="seg__label seg__label--shown" x="26.00%" y="18" text-anchor="middle">52%</text>"#
        ), "{first}");
        assert!(first.contains(
            r#"<rect class="seg seg--budget" x="52.00%" y="0" width="31.00%" height="28" rx="4"/><text class="seg__label seg__label--budget" x="67.50%" y="18" text-anchor="middle">31%</text>"#
        ));
        assert!(first.contains(
            r#"<rect class="seg seg--rank" x="83.00%" y="0" width="17.00%" height="28" rx="4"/><text class="seg__label seg__label--rank" x="91.50%" y="18" text-anchor="middle">17%</text>"#
        ));
        assert!(first
            .contains(">Lost to budget: 31% of the estimated eligible Search impressions</text>"));
        assert_eq!(first.matches(r#"class="pt" tabindex="0""#).count(), 3);

        let brand = &chart.rows[2].svg;
        assert!(brand.contains(r#"<rect class="seg seg--budget" x="91.00%" y="0" width="2.00%""#));
        assert!(!brand.contains(">2%</text>"));
        assert!(brand.contains(
            r#"<text class="seg__label seg__label--rank wide" x="96.50%" y="18" text-anchor="middle">7%</text>"#
        ), "{brand}");

        let table = chart.table.unwrap();
        assert_eq!(
            table.headers,
            [
                "Campaign",
                "Impression share",
                "Lost to budget",
                "Lost to ad rank",
                "Days with an exact share"
            ]
        );
        assert_eq!(
            table.rows[1],
            ["Search - Implants", "64%", "6%", "30%", "5 of 7"]
        );
        assert_eq!(
            chart
                .keys
                .iter()
                .map(|key| (key.kind, key.label.as_str()))
                .collect::<Vec<_>>(),
            [
                ("shown", "Impression share"),
                ("budget", "Lost to budget"),
                ("rank", "Lost to ad rank")
            ]
        );
    }

    #[test]
    fn the_sentence_names_a_loss_to_ad_rank_when_it_is_the_largest_and_states_the_days_it_uses() {
        let chart = search_share(
            "Search impression share, last 7 days",
            vec![
                row("Search - Emergency", Some((0.52, 0.21, 0.27)), 7, 0),
                row("Search - Implants", Some((0.64, 0.06, 0.30)), 4, 2),
            ],
        );
        assert_eq!(
            chart.sentence.words(),
            "Search - Implants lost 30% of its estimated eligible Search impressions to ad rank. That is the largest loss in this chart. Google Ads gives an exact share for 4 of its 7 days, and these shares use only those days. On 2 of the 7 days, Google Ads reports values outside its exact reporting range. Those days are excluded."
        );
    }

    #[test]
    fn each_share_is_drawn_at_its_own_width_when_the_three_do_not_add_up_to_all() {
        let chart = search_share(
            "Search impression share, last 7 days",
            vec![row("Search - Brand", Some((0.5, 0.2, 0.2)), 7, 0)],
        );
        let bar = &chart.rows[0].svg;
        for expected in [
            r#"<rect class="seg seg--shown" x="0.00%" y="0" width="50.00%""#,
            r#"<rect class="seg seg--budget" x="50.00%" y="0" width="20.00%""#,
            r#"<rect class="seg seg--rank" x="70.00%" y="0" width="20.00%""#,
        ] {
            assert!(bar.contains(expected), "{expected}\n{bar}");
        }
        let over = search_share(
            "Search impression share, last 7 days",
            vec![row("Search - Brand", Some((0.7, 0.2, 0.2)), 7, 0)],
        );
        assert!(
            over.rows[0]
                .svg
                .contains(r#"<rect class="seg seg--rank" x="90.00%" y="0" width="10.00%""#),
            "{}",
            over.rows[0].svg
        );
    }

    #[test]
    fn under_ten_percent_is_never_a_number() {
        let chart = search_share(
            "Searches",
            vec![
                row("Search - New", None, 0, 7),
                row("Search - Brand", Some((0.995, 0.003, 0.002)), 6, 1),
            ],
        );
        assert_eq!(
            chart.sentence.words(),
            "Campaigns with exact shares lost under 1% of estimated eligible Search impressions to budget and under 1% to ad rank."
        );
        assert_eq!(
            chart.rows[0].svg,
            r#"<svg class="chart__bars" height="28" role="img" aria-label="Search - New: no exact impression share"><text x="0" y="18">No exact impression share</text></svg>"#
        );
        assert_eq!(
            chart.table.unwrap().rows[0],
            [
                "Search - New",
                "Under 10%",
                "Not available",
                "Not available",
                "0 of 7"
            ]
        );

        let alone = search_share(
            "Searches",
            vec![row("Search - Brand", Some((0.995, 0.003, 0.002)), 6, 1)],
        );
        assert_eq!(
            alone.sentence.words(),
            "Search - Brand appeared for 100% of its estimated eligible Search impressions. It lost under 1% to budget and under 1% to ad rank. Google Ads gives an exact share for 6 of its 7 days, and these shares use only those days. On 1 of the 7 days, Google Ads reports values outside its exact reporting range. Those days are excluded."
        );

        let none = search_share("Searches", vec![row("Search - New", None, 0, 7)]);
        assert_eq!(
            none.sentence.words(),
            "Google Ads reports under 10% for each of these days and gives no exact share."
        );

        let empty = search_share("Searches", vec![]);
        assert_eq!(
            empty.sentence.words(),
            "Google Ads has no Search impression share for these days."
        );
        assert!(empty.table.is_none());
    }

    #[test]
    fn days_without_a_share_do_not_say_under_ten_percent() {
        let said = |rows| search_share("Searches", rows).sentence.words();
        assert_eq!(
            said(vec![row("Search - New", None, 0, 7)]),
            "Google Ads reports under 10% for each of these days and gives no exact share."
        );
        assert_eq!(
            said(vec![row("Search - New", None, 0, 0)]),
            "Google Ads reports no Search impression share for these days."
        );
        assert_eq!(
            said(vec![
                row("Search - New", None, 0, 0),
                row("Search - Old", None, 0, 0)
            ]),
            "Google Ads reports no Search impression share for these days."
        );
        assert_eq!(
            said(vec![row("Search - New", None, 0, 3)]),
            "Google Ads reports no exact Search impression share for these days."
        );
        assert_eq!(
            said(vec![
                row("Search - New", None, 0, 7),
                row("Search - Old", None, 0, 0)
            ]),
            "Google Ads reports no exact Search impression share for these days."
        );
        let unknown = search_share("Searches", vec![row("Search - New", None, 0, 0)]);
        assert_eq!(
            unknown.table.unwrap().rows[0],
            [
                "Search - New",
                "Not available",
                "Not available",
                "Not available",
                "0 of 7"
            ]
        );
    }
    #[test]
    fn missing_days_do_not_turn_a_partial_share_into_a_whole_window_claim() {
        let chart = search_share("Searches", vec![row("Search - New", None, 0, 3)]);
        assert_eq!(chart.table.unwrap().rows[0][1], "Not available");
        let chart = search_share(
            "Searches",
            vec![
                row("Search - Unknown", None, 0, 7),
                row("Search - Exact", Some((0.992, 0.004, 0.004)), 7, 0),
            ],
        );
        assert_eq!(chart.sentence.words(), "Campaigns with exact shares lost under 1% of estimated eligible Search impressions to budget and under 1% to ad rank.");
        let chart = search_share(
            "Searches",
            vec![row("Search - Partial", Some((0.5, 0.3, 0.2)), 4, 2)],
        );
        assert_eq!(chart.sentence.words(), "Search - Partial lost 30% of its estimated eligible Search impressions to budget. That is the largest loss in this chart. Google Ads gives an exact share for 4 of its 7 days, and these shares use only those days. On 2 of the 7 days, Google Ads reports values outside its exact reporting range. Those days are excluded.");
    }
}
