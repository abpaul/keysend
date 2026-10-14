use std::fmt::Write as _;

use super::{chars, esc, Chart, Key, PRINT_PLOT};

const WIDTH: f64 = 640.0;
const NAME_COLUMN: f64 = 170.0;
const SHARES_STEP: f64 = 42.0;
const STACK_STEP: f64 = 58.0;
const GRID_LABELS: f64 = 36.0;
const GRID_HEIGHT: f64 = 162.0;
const KEY_CHAR: f64 = 6.6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Frame {
    Trend,
    Weeks,
    Shares,
    Stack,
    Grid,
}

const STYLE: &str = "text{fill:#5C5470;font-family:'Liberation Sans';font-size:12px}.grid{stroke:#E4E2EC;stroke-width:1}.band{fill:#E7E0F5}.line{fill:none;stroke:#5126A6;stroke-width:2;stroke-linejoin:round}.change{stroke:#292039;stroke-width:1.5;stroke-dasharray:3 3}.target{stroke:#292039;stroke-width:1.5}.chart__change,.chart__target,.chart__value{paint-order:stroke;stroke:#FFFFFF;stroke-width:4px;stroke-linejoin:round}.chart__change,.chart__target{fill:#292039;font-weight:bold}.chart__value--in{fill:#FFFFFF;stroke:none}.bar--spend{fill:#5126A6}.bar--results{fill:#D6ED79}.bar--above{fill:#8F78CC}.bar--open{fill:none;stroke:#5126A6;stroke-width:1.5;stroke-dasharray:4 3}.name{fill:#292039;font-size:13px;font-weight:bold}.narrow,.tip,.pt__hit,.pt__dot{display:none}.pt__dot--out{display:inline;fill:#A8201A;stroke:#FFFFFF;stroke-width:1.5}.pt__dot--lone{display:inline;fill:#5126A6}.k--line,.k--spend{fill:#5126A6}.k--band{fill:#E7E0F5}.k--outside{fill:#A8201A}.k--results{fill:#D6ED79}.k--above{fill:#8F78CC}.k--open{fill:none;stroke:#5126A6;stroke-width:1.5;stroke-dasharray:4 3}.k--change{stroke:#292039;stroke-width:1.5;stroke-dasharray:3 3}.detail{font-size:11px}.seg{stroke:#FFFFFF;stroke-width:2}.seg--shown,.seg--act-0,.k--shown,.k--act-0{fill:#5126A6}.seg--budget,.seg--act-1,.k--budget,.k--act-1{fill:#8F78CC}.seg--act-2,.k--act-2{fill:#D6ED79}.seg--act-3,.k--act-3{fill:#292039}.seg--rank,.seg--act-rest,.k--rank,.k--act-rest{fill:#D9D5E6}.seg__label{fill:#FFFFFF;font-weight:bold}.seg__label--rank{fill:#292039}.cell{stroke:#FFFFFF;stroke-width:2}.cell--0,.k--cell-0{fill:#E4E2EC}.cell--1,.k--cell-1{fill:#D9CCF2}.cell--2{fill:#B39DE0}.cell--3{fill:#8F78CC}.cell--4,.k--cell-4{fill:#5126A6}";

fn inner(svg: &str) -> &str {
    let start = svg.find('>').map_or(0, |at| at + 1);
    let end = svg.rfind("</svg>").unwrap_or(svg.len());
    &svg[start..end.max(start)]
}

fn name_lines(name: &str) -> Vec<String> {
    let fits = (NAME_COLUMN / 7.4) as usize;
    let letters = name.chars().collect::<Vec<_>>();
    if letters.len() <= fits {
        return vec![name.into()];
    }
    let cut = letters[..=fits]
        .iter()
        .rposition(|letter| letter.is_whitespace())
        .filter(|at| *at > 0)
        .unwrap_or(fits);
    let first = letters[..cut].iter().collect::<String>();
    let rest = letters[cut..].iter().collect::<String>();
    let rest = rest.trim();
    let second = if rest.chars().count() <= fits {
        rest.to_owned()
    } else {
        format!(
            "{}…",
            rest.chars().take(fits - 1).collect::<String>().trim_end()
        )
    };
    vec![first.trim_end().to_owned(), second]
}

fn rows(chart: &Chart, step: f64, bar: f64) -> (String, f64) {
    let mut body = String::new();
    for (index, row) in chart.rows.iter().enumerate() {
        let top = index as f64 * step;
        let lines = name_lines(&row.name);
        let first = if lines.len() == 1 { 18.0 } else { 13.0 };
        for (line, text) in lines.iter().enumerate() {
            let _ = write!(
                body,
                r#"<text class="name" x="0" y="{:.0}">{}</text>"#,
                top + first + line as f64 * 14.0,
                esc(text)
            );
        }
        let _ = write!(
            body,
            r#"<svg x="{NAME_COLUMN:.0}" y="{top:.0}" width="{:.0}" height="{bar:.0}" overflow="visible">{}</svg>"#,
            WIDTH - NAME_COLUMN - 44.0,
            inner(&row.svg)
        );
        if let Some(values) = &row.values {
            let _ = write!(
                body,
                r#"<text class="detail" x="{NAME_COLUMN:.0}" y="{:.0}">{}</text>"#,
                top + bar + 14.0,
                esc(values)
            );
        }
    }
    (body, chart.rows.len() as f64 * step)
}

fn keys(keys: &[Key], top: f64) -> (String, f64) {
    let mut out = String::new();
    let (mut x, mut y) = (0.0, top);
    for key in keys {
        let width = chars(&key.label) * KEY_CHAR + 32.0;
        if x > 0.0 && x + width > WIDTH {
            x = 0.0;
            y += 18.0;
        }
        let mark = match key.kind {
            "change" => format!(
                r#"<line class="k--change" x1="{0:.0}" x2="{0:.0}" y1="{1:.0}" y2="{2:.0}"/>"#,
                x + 5.0,
                y - 1.0,
                y + 11.0
            ),
            kind => format!(
                r#"<rect class="k--{kind}" x="{x:.0}" y="{y:.0}" width="10" height="10" rx="{}"/>"#,
                if kind == "outside" { 5 } else { 3 }
            ),
        };
        let _ = write!(
            out,
            r#"{mark}<text x="{:.0}" y="{:.0}">{}</text>"#,
            x + 16.0,
            y + 9.0,
            esc(&key.label)
        );
        x += width;
    }
    (out, if keys.is_empty() { top } else { y + 16.0 })
}

pub fn standalone(chart: &Chart, frame: Frame) -> Option<String> {
    chart.table.as_ref()?;
    let (body, height) = match (frame, &chart.svg) {
        (Frame::Shares, _) => rows(chart, SHARES_STEP, 29.0),
        (Frame::Stack, _) => rows(chart, STACK_STEP, 28.0),
        (Frame::Trend, Some(svg)) => (
            format!(
                r#"<svg x="30" y="6" width="{PRINT_PLOT:.0}" height="190" overflow="visible">{}</svg>"#,
                inner(svg)
            ),
            196.0,
        ),
        (Frame::Weeks, Some(svg)) => (
            format!(
                r#"<svg x="30" y="10" width="{:.0}" height="170" overflow="visible">{}</svg>"#,
                WIDTH - 30.0,
                inner(svg)
            ),
            180.0,
        ),
        (Frame::Grid, Some(svg)) => (
            format!(
                r#"<svg x="{GRID_LABELS:.0}" y="0" width="{:.0}" height="{GRID_HEIGHT:.0}" overflow="visible">{}</svg>"#,
                WIDTH - GRID_LABELS - 4.0,
                inner(svg)
            ),
            GRID_HEIGHT,
        ),
        (_, None) => return None,
    };
    let (keys, height) = keys(&chart.keys, height + 4.0);
    Some(format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{WIDTH:.0}" height="{height:.0}" viewBox="0 0 {WIDTH:.0} {height:.0}"><style>{STYLE}</style>{body}{keys}</svg>"#
    ))
}

#[cfg(test)]
mod tests {
    use super::super::english_calls::{hour_grid, search_share, shares, trend_for_print};
    use super::*;
    use crate::monitoring::charts::{Part, SearchShare, TrendDay, TrendMetric};

    #[test]
    fn shares_become_one_document_with_a_name_column_and_keys() {
        let chart = shares(
            "By Campaign",
            "Campaign",
            vec![
                Part {
                    name: "Search - Emergency & a very long Campaign name".into(),
                    href: None,
                    spend: 300.0,
                    results: 10.0,
                },
                Part {
                    name: "Search - Brand".into(),
                    href: None,
                    spend: 100.0,
                    results: 10.0,
                },
            ],
            "clicks",
            "USD",
        );
        let svg = standalone(&chart, Frame::Shares).unwrap();
        assert!(svg.starts_with(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="640" height="104" viewBox="0 0 640 104"><style>text{fill:#5C5470;"#
        ), "{svg}");
        assert!(svg.contains(
            r#"</style><text class="name" x="0" y="13">Search - Emergency &amp; a</text><text class="name" x="0" y="27">very long Campaign na…</text><svg x="170" y="0" width="426" height="29" overflow="visible"><g class="pt" tabindex="0" role="img"><title>"#
        ), "{svg}");
        assert!(svg.contains(
            r#"<text class="name" x="0" y="60">Search - Brand</text><svg x="170" y="42" width="426" height="29" overflow="visible">"#
        ));
        assert!(svg.ends_with(
            r#"<rect class="k--spend" x="0" y="88" width="10" height="10" rx="3"/><text x="16" y="97">Share of spend</text><rect class="k--results" x="124" y="88" width="10" height="10" rx="3"/><text x="140" y="97">Share of clicks</text></svg>"#
        ), "{svg}");
        assert_eq!(svg.matches("<svg").count(), 3);
        assert_eq!(svg.matches("</svg>").count(), 3);
    }

    #[test]
    fn a_stack_has_its_values_in_words_under_each_bar_and_a_grid_has_its_legend() {
        let stack = search_share(
            "Searches that the ads did not appear on, last 7 days",
            vec![
                SearchShare {
                    name: "Search - Emergency".into(),
                    href: None,
                    shares: Some((0.52, 0.31, 0.17)),
                    limit_days: 0,
                    days: 7,
                },
                SearchShare {
                    name: "Search - Brand".into(),
                    href: None,
                    shares: Some((0.91, 0.02, 0.07)),
                    limit_days: 0,
                    days: 7,
                },
            ],
        );
        let svg = standalone(&stack, Frame::Stack).unwrap();
        assert!(svg.contains(
            r#"<text class="detail" x="170" y="42">Shown 52% · Missed: budget 31% · Missed: ad rank 17%</text>"#
        ), "{svg}");
        assert!(svg.contains(
            r#"<text class="detail" x="170" y="100">Shown 91% · Missed: budget 2% · Missed: ad rank 7%</text>"#
        ), "{svg}");
        assert!(
            svg.contains(r#"<text x="16" y="129">Shown</text>"#),
            "{svg}"
        );

        let mut values = [[0.0; 24]; 7];
        values[0][9] = 8.0;
        let grid = hour_grid(&values, "clicks");
        let svg = standalone(&grid, Frame::Grid).unwrap();
        assert!(
            svg.contains(r#"<svg x="36" y="0" width="600" height="162" overflow="visible">"#),
            "{svg}"
        );
        assert!(svg.contains(r#"<text x="-34" y="13">Mon</text>"#));
        assert_eq!(svg.matches(r#"<rect class="cell "#).count(), 168);
        assert!(
            svg.contains(r#"<text x="219" y="175">More clicks</text>"#),
            "{svg}"
        );
    }

    #[test]
    fn a_trend_for_print_has_its_line_in_pixels_and_an_empty_chart_has_no_document() {
        let days = (0..5)
            .map(|index| TrendDay {
                day: 20_733 + index,
                value: Some(500.0),
                range: Some((200.0, 800.0)),
                unusual: false,
            })
            .collect::<Vec<_>>();
        let chart = trend_for_print(TrendMetric::Spend, &days, &[], "USD").unwrap();
        let svg = standalone(&chart, Frame::Trend).unwrap();
        assert!(svg.contains(
            r#"<svg x="30" y="6" width="600" height="190" overflow="visible"><path class="band" d="M0.0,0.0L150.0,0.0L300.0,0.0L450.0,0.0L600.0,0.0L600.0,150.0L450.0,150.0L300.0,150.0L150.0,150.0L0.0,150.0Z"/>"#
        ), "{svg}");
        assert!(svg.contains(
            r#"<path class="line" d="M0.0,75.0L150.0,75.0L300.0,75.0L450.0,75.0L600.0,75.0" vector-effect="non-scaling-stroke"/>"#
        ));
        assert!(!svg.contains("preserveAspectRatio"));
        assert!(svg.contains(r#"<rect class="k--band" x="65" y="200" width="10" height="10" rx="3"/><text x="81" y="209">Usual range for the weekday</text>"#), "{svg}");

        let none = shares("By device", "device", vec![], "clicks", "USD");
        assert_eq!(standalone(&none, Frame::Shares), None);
    }
}
