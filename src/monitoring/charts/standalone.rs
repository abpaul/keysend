use std::fmt::Write as _;

use super::{chars, esc, Chart, Key, PRINT_PLOT};

const WIDTH: f64 = 640.0;
const NAME_COLUMN: f64 = 220.0;
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
    let mut top = 0.0;
    for row in &chart.rows {
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
        let mut details = Vec::<String>::new();
        if let Some(values) = &row.values {
            for value in values.split(" · ") {
                match details.last_mut() {
                    Some(line)
                        if (chars(line) + chars(value) + 3.0) * 6.0 <= WIDTH - NAME_COLUMN =>
                    {
                        line.push_str(" · ");
                        line.push_str(value);
                    }
                    _ => details.push(value.into()),
                }
            }
        }
        for (index, line) in details.iter().enumerate() {
            let _ = write!(
                body,
                r#"<text class="detail" x="{NAME_COLUMN:.0}" y="{:.0}">{}</text>"#,
                top + bar + (index + 1) as f64 * 14.0,
                esc(line)
            );
        }
        top += step.max(bar + details.len() as f64 * 14.0 + 8.0);
    }
    (body, top)
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

pub fn standalone_spark(svg: &str) -> String {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 32" preserveAspectRatio="none"><style>path{{fill:none;stroke:#5126A6;stroke-width:2;stroke-linejoin:round;stroke-linecap:round}}</style>{}</svg>"#,
        inner(svg)
    )
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
            r#"</style><text class="name" x="0" y="13">Search - Emergency &amp; a very</text><text class="name" x="0" y="27">long Campaign name</text><svg x="220" y="0" width="376" height="29" overflow="visible"><g class="pt" tabindex="0" role="img" aria-label="#
        ), "{svg}");
        assert!(svg.contains(
            r#"<text class="name" x="0" y="60">Search - Brand</text><svg x="220" y="42" width="376" height="29" overflow="visible">"#
        ));
        assert!(svg.ends_with(
            r#"<rect class="k--spend" x="0" y="88" width="10" height="10" rx="3"/><text x="16" y="97">Share of spend</text><rect class="k--results" x="124" y="88" width="10" height="10" rx="3"/><text x="140" y="97">Share of clicks</text></svg>"#
        ), "{svg}");
        assert_eq!(svg.matches("<svg").count(), 3);
        assert_eq!(svg.matches("</svg>").count(), 3);
    }

    #[test]
    fn a_name_takes_one_line_up_to_29_letters_and_ends_with_an_ellipsis_after_two() {
        assert_eq!(
            name_lines("Popesti-Leordeni, Ilfov Count"),
            ["Popesti-Leordeni, Ilfov Count"]
        );
        assert_eq!(
            name_lines("Website traffic-Search-1 Clicks"),
            ["Website traffic-Search-1", "Clicks"]
        );
        assert_eq!(
            name_lines("Performance Max - All products in every country - Spring sale"),
            ["Performance Max - All", "products in every country -…"]
        );
    }

    #[test]
    fn translated_stack_values_wrap_without_clipping_or_overlapping_the_next_campaign() {
        let text = crate::language::Language::Ro.text().unwrap();
        let row = |name: &str| SearchShare {
            name: name.into(),
            href: None,
            shares: Some((0.60, 0.25, 0.15)),
            exact_days: 7,
            limit_days: 0,
            days: 7,
        };
        let chart =
            super::super::search_share(text, "Cota de afișări", vec![row("Prima"), row("A doua")])
                .unwrap();
        let svg = standalone(&chart, Frame::Stack).unwrap();
        assert!(svg.contains(r#"<text class="detail" x="220" y="42">Cota de afișări 60 % · Pierdută din cauza bugetului 25 %</text>"#), "{svg}");
        assert!(svg.contains(r#"<text class="detail" x="220" y="56">Pierdută din cauza clasării anunțului 15 %</text>"#), "{svg}");
        assert!(svg.contains(r#"<svg x="220" y="64""#), "{svg}");
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
                    exact_days: 7,
                    limit_days: 0,
                    days: 7,
                },
                SearchShare {
                    name: "Search - Brand".into(),
                    href: None,
                    shares: Some((0.91, 0.02, 0.07)),
                    exact_days: 7,
                    limit_days: 0,
                    days: 7,
                },
            ],
        );
        let svg = standalone(&stack, Frame::Stack).unwrap();
        assert!(svg.contains(
            r#"<text class="detail" x="220" y="42">Impression share 52% · Lost to budget 31% · Lost to ad rank 17%</text>"#
        ), "{svg}");
        assert!(svg.contains(
            r#"<text class="detail" x="220" y="100">Impression share 91% · Lost to budget 2% · Lost to ad rank 7%</text>"#
        ), "{svg}");
        assert!(
            svg.contains(r#"<text x="16" y="129">Impression share</text>"#),
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
