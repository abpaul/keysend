use std::fmt::Write as _;

use loco_rs::Result;

use super::{esc, share, share_text, shares, tip, Chart, Key, Part, Sentence, Subject, Table};
use crate::language::{fill, say, say_plural, Text};
use crate::monitoring::tracking::MIN_CONVERSIONS;

pub const MIN_GRID_COUNT: f64 = 100.0;
pub const MIN_KNOWN_SHARE: f64 = 0.8;
pub const LOW_SCORE: i64 = 4;

const BLOCK_HOURS: usize = 3;
const ROW: f64 = 20.0;
const CELL: f64 = 18.0;
const COLUMN: f64 = 100.0 / 24.0;
fn hour(hour: usize) -> String {
    format!("{:02}:00", hour % 24)
}

pub fn hour_grid(text: &Text, values: &[[f64; 24]; 7], judged: bool) -> Result<Chart> {
    let w = &text.written;
    let (results, heading) = if judged {
        (w.results_conversions, w.metric_conversions)
    } else {
        (w.results_clicks, w.metric_clicks)
    };
    let count = |value: f64| text.count(value);
    let title = say!(text, hours_title, results = heading)?;
    let total = values.iter().flatten().sum::<f64>();
    let largest = values.iter().flatten().copied().fold(0.0, f64::max);
    if total <= 0.0 {
        return Ok(Chart::empty(
            title,
            fill("hours_none", w.missing_hours_none, &[("results", results)])?,
        ));
    }
    let blocks = (0..24 / BLOCK_HOURS)
        .map(|block| {
            values
                .iter()
                .map(|day| {
                    day[block * BLOCK_HOURS..(block + 1) * BLOCK_HOURS]
                        .iter()
                        .sum::<f64>()
                })
                .sum::<f64>()
        })
        .collect::<Vec<_>>();
    let most = blocks.iter().copied().fold(f64::MIN, f64::max);
    let fewest = blocks.iter().copied().fold(f64::MAX, f64::min);
    let with = |value: f64| {
        blocks
            .iter()
            .enumerate()
            .filter(|(_, block)| **block == value)
            .map(|(index, _)| index)
            .collect::<Vec<_>>()
    };
    let (best, weakest) = (with(most), with(fewest));
    let between = |block: usize| {
        say!(
            text,
            hours_between,
            from = hour(block * BLOCK_HOURS),
            to = hour((block + 1) * BLOCK_HOURS)
        )
    };
    let zone = w.explain_hours_zone;
    let sentence = if total >= MIN_GRID_COUNT && most > fewest {
        let low = match weakest.as_slice() {
            [only] => say!(
                text,
                hours_fewest_one,
                between = between(*only)?,
                count = count(fewest)
            )?,
            tied => say_plural!(
                text,
                hours_fewest_tied,
                tied.len() as u64,
                hours = BLOCK_HOURS,
                each = count(fewest)
            )?,
        };
        let (lead, mark, counts) = match best.as_slice() {
            [only] => (
                say!(text, hours_most_lead, results = results)?,
                between(*only)?,
                say!(
                    text,
                    hours_most_of,
                    most = count(most),
                    total = count(total)
                )?,
            ),
            tied => (
                String::new(),
                say_plural!(
                    text,
                    hours_most_tied,
                    tied.len() as u64,
                    hours = BLOCK_HOURS,
                    results = results
                )?,
                say!(text, hours_each, most = count(most))?,
            ),
        };
        Sentence {
            strong: String::new(),
            text: lead,
            mark,
            after: say!(text, hours_after, counts = counts, low = low, zone = zone)?,
        }
    } else {
        Sentence::plain(say!(
            text,
            hours_too_few,
            count = count(total),
            results = results,
            zone = zone
        )?)
    };

    let height = ROW * 7.0 + 22.0;
    let mut svg = format!(
        r#"<svg class="chart__svg" height="{height:.0}" role="group" aria-label="{}">"#,
        esc(&title)
    );
    let mut rows = Vec::with_capacity(7);
    for (index, day) in values.iter().enumerate() {
        let (short, long) = (w.format.weekdays_short[index], w.format.weekdays[index]);
        let top = index as f64 * ROW;
        let sum = day.iter().sum::<f64>();
        let peak = (0..24)
            .rev()
            .max_by(|a, b| day[*a].total_cmp(&day[*b]))
            .unwrap_or_default();
        let line = if sum > 0.0 {
            say!(
                text,
                hours_day_line,
                day = long,
                sum = count(sum),
                results = results,
                hour = hour(peak),
                peak = count(day[peak])
            )?
        } else {
            say!(text, hours_day_none, day = long, results = results)?
        };
        let _ = write!(
            svg,
            r#"<text x="-34" y="{:.0}">{short}</text><g class="pt" tabindex="0" role="img" aria-label="{}."><rect class="pt__hit" x="0" y="{top:.0}" width="100%" height="{CELL:.0}"/>"#,
            top + 13.0,
            esc(&line)
        );
        for (at, value) in day.iter().enumerate() {
            let level = if *value <= 0.0 {
                0
            } else {
                ((value / largest * 4.0).ceil() as usize).clamp(1, 4)
            };
            let _ = write!(
                svg,
                r#"<rect class="cell cell--{level}" x="{:.2}%" y="{top:.0}" width="{COLUMN:.2}%" height="{CELL:.0}" rx="3"><title>{}</title></rect>"#,
                at as f64 * COLUMN,
                say!(
                    text,
                    hours_cell,
                    short = short,
                    hour = hour(at),
                    count = count(*value),
                    results = results
                )?
            );
        }
        let _ = write!(svg, "{}</g>", tip(0.0, top - 28.0, &[line], false));
        let mut row = vec![long.to_owned()];
        row.extend(day.iter().map(|value| count(*value)));
        rows.push(row);
    }
    for at in [0, 6, 12, 18] {
        let _ = write!(
            svg,
            r#"<text x="{:.2}%" y="{:.0}">{}</text>"#,
            at as f64 * COLUMN,
            ROW * 7.0 + 14.0,
            hour(at)
        );
    }
    svg.push_str("</svg>");
    let mut headers = vec![w.hours_header_weekday.to_owned()];
    headers.extend((0..24).map(hour));
    Ok(Chart {
        title,
        sentence,
        svg: Some(svg),
        rows: Vec::new(),
        keys: vec![
            Key {
                kind: "cell-0",
                label: say!(text, hours_key_none, results = results)?,
            },
            Key {
                kind: "cell-1",
                label: say!(text, hours_key_fewer, results = results)?,
            },
            Key {
                kind: "cell-4",
                label: say!(text, hours_key_more, results = results)?,
            },
        ],
        table: Some(Table {
            caption: say!(text, hours_caption, results = heading)?,
            headers,
            rows,
            ..Default::default()
        }),
    })
}

pub struct Group {
    pub name: String,
    pub known: bool,
    pub spend: f64,
    pub clicks: f64,
    pub results: f64,
}

pub fn demographic(
    text: &Text,
    title: &str,
    subject: Subject,
    groups: Vec<Group>,
    judged: bool,
    currency: &str,
) -> Result<Chart> {
    let clicks = groups.iter().map(|group| group.clicks).sum::<f64>();
    if clicks <= 0.0 {
        return Ok(Chart::empty(
            title.into(),
            match subject {
                Subject::Gender => text.written.demographic_gender_none,
                _ => text.written.demographic_age_none,
            },
        ));
    }
    let known = groups
        .iter()
        .filter(|group| group.known)
        .map(|group| group.clicks)
        .sum::<f64>();
    if known / clicks < MIN_KNOWN_SHARE {
        return Ok(Chart::empty(title.into(), {
            let share = share_text(text, share(known, clicks));
            match subject {
                Subject::Gender => say!(text, demographic_gender_thin, share = share)?,
                _ => say!(text, demographic_age_thin, share = share)?,
            }
        }));
    }
    shares(
        text,
        subject,
        title,
        groups
            .into_iter()
            .map(|group| Part {
                name: group.name,
                href: None,
                spend: group.spend,
                results: group.results,
            })
            .collect(),
        judged,
        currency,
    )
}

pub fn score_bar(text: &Text, score: i64) -> Result<String> {
    let score = score.clamp(1, 10);
    Ok(format!(
        r#"<svg class="score" viewBox="0 0 118 8" width="118" height="8" role="img" aria-label="{}"><path class="score__track" d="M0 4h118"/><path class="score__value" d="M0 4h{}"/></svg>"#,
        say!(text, deep_score_aria, score = score)?,
        score * 12 - 2
    ))
}

pub fn meter(share: f64, mark: Option<f64>, label: &str) -> String {
    let percent = |part: f64| part.clamp(0.0, 1.0) * 100.0;
    let mark = mark.map_or_else(String::new, |mark| {
        format!(
            r#"<rect class="meter__mark" x="{:.2}%" y="-3" width="2" height="14"/>"#,
            percent(mark)
        )
    });
    format!(
        r#"<svg class="meter" width="100%" height="8" role="img" aria-label="{}"><rect class="meter__track" width="100%" height="8" rx="4"/><rect class="meter__value" width="{:.2}%" height="8" rx="4"/>{mark}</svg>"#,
        esc(label),
        percent(share)
    )
}

pub fn quality_sentence(text: &Text, scores: &[Option<i64>]) -> Result<String> {
    let known = scores.iter().flatten().copied().collect::<Vec<_>>();
    match known.as_slice() {
        [] => Ok(text.written.deep_quality_none.into()),
        [score] => say!(text, deep_quality_one, score = score),
        known => {
            let low = known.iter().filter(|score| **score <= LOW_SCORE).count();
            say_plural!(
                text,
                deep_quality_low,
                low as u64,
                total = known.len(),
                limit = LOW_SCORE
            )
        }
    }
}

pub fn delay_sentence(text: &Text, conversions: f64, late: f64) -> Result<Option<String>> {
    let percent = share(late, conversions).round();
    (conversions >= MIN_CONVERSIONS && percent >= 1.0)
        .then(|| say!(text, delay_note, percent = share_text(text, percent)))
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::super::english_calls::{demographic, hour_grid};
    use super::*;

    fn grid(scale: f64) -> [[f64; 24]; 7] {
        let mut values = [[0.0; 24]; 7];
        for day in &mut values[..5] {
            day[9] = 8.0 * scale;
            day[10] = 4.0 * scale;
            day[14] = 2.0 * scale;
        }
        values[5][20] = scale;
        values
    }

    #[test]
    fn the_grid_has_one_cell_for_each_hour_and_names_the_best_and_the_weakest_hours() {
        let chart = hour_grid(&grid(4.0), "clicks");
        assert_eq!(chart.title, "Clicks by hour and weekday, 30 days");
        assert_eq!(
            chart.sentence.words(),
            "Most clicks come between 09:00 and 12:00: 240 of 284. 5 blocks of 3 hours have the fewest: 0 each. Hours are in the time zone of the Ad Account."
        );
        assert_eq!(chart.sentence.mark, "between 09:00 and 12:00");
        let svg = chart.svg.unwrap();
        assert!(svg.starts_with(
            r#"<svg class="chart__svg" height="162" role="group" aria-label="Clicks by hour and weekday, 30 days"><text x="-34" y="13">Mon</text><g class="pt" tabindex="0" role="img" aria-label="Monday: 56 clicks, the most at 09:00 (32)."><rect class="pt__hit" x="0" y="0" width="100%" height="18"/><rect class="cell cell--0" x="0.00%" y="0" width="4.17%" height="18" rx="3"><title>Mon 00:00: 0 clicks</title></rect>"#
        ), "{svg}");
        assert!(svg.contains(
            r#"<rect class="cell cell--4" x="37.50%" y="0" width="4.17%" height="18" rx="3"><title>Mon 09:00: 32 clicks</title></rect><rect class="cell cell--2" x="41.67%" y="0" width="4.17%" height="18" rx="3"><title>Mon 10:00: 16 clicks</title></rect>"#
        ));
        assert!(svg.contains(
            r#"<rect class="cell cell--1" x="83.33%" y="100" width="4.17%" height="18" rx="3"><title>Sat 20:00: 4 clicks</title></rect>"#
        ));
        assert!(svg.contains(r#"aria-label="Sunday: no clicks.""#));
        assert_eq!(svg.matches(r#"<rect class="cell "#).count(), 168);
        assert_eq!(svg.matches(r#"class="pt" tabindex="0""#).count(), 7);
        assert!(svg.contains(r#"<text x="50.00%" y="154">12:00</text>"#));
        let table = chart.table.unwrap();
        assert_eq!(table.headers.len(), 25);
        assert_eq!(table.headers[10], "09:00");
        assert_eq!(table.rows[0][0], "Monday");
        assert_eq!(table.rows[0][10], "32");
        assert_eq!(table.rows[5][21], "4");
        assert_eq!(
            chart
                .keys
                .iter()
                .map(|key| (key.kind, key.label.as_str()))
                .collect::<Vec<_>>(),
            [
                ("cell-0", "No clicks"),
                ("cell-1", "Fewer clicks"),
                ("cell-4", "More clicks")
            ]
        );
    }

    #[test]
    fn a_thin_grid_claims_no_difference_and_an_empty_grid_is_one_sentence() {
        let thin = hour_grid(&grid(1.0), "conversions");
        assert_eq!(
            thin.sentence.words(),
            "71 conversions in 30 days are too few to name the best and the weakest hours. Hours are in the time zone of the Ad Account."
        );
        assert!(thin.svg.is_some());

        let mut edge = [[0.0; 24]; 7];
        edge[0][9] = MIN_GRID_COUNT;
        assert!(hour_grid(&edge, "clicks")
            .sentence
            .words()
            .starts_with("Most clicks come between 09:00 and 12:00: 100 of 100."));

        let mut spread = [[10.0; 24]; 7];
        spread[2][4] = 1.0;
        assert_eq!(
            hour_grid(&spread, "clicks").sentence.words(),
            "7 blocks of 3 hours have the most clicks: 210 each. The fewest come between 03:00 and 06:00: 201. Hours are in the time zone of the Ad Account."
        );

        let none = hour_grid(&[[0.0; 24]; 7], "clicks");
        assert_eq!(none.svg, None);
        assert_eq!(
            none.sentence.words(),
            "No hour had clicks in these 30 days."
        );
    }

    fn group(name: &str, known: bool, clicks: f64) -> Group {
        Group {
            name: name.into(),
            known,
            spend: clicks * 2.0,
            clicks,
            results: clicks / 10.0,
        }
    }

    #[test]
    fn age_and_gender_need_a_known_value_for_four_fifths_of_clicks() {
        let title = "Spend and conversions by age group, 30 days";
        let thin = demographic(
            title,
            "age group",
            "age",
            vec![group("25 to 34", true, 79.0), group("Unknown", false, 21.0)],
            "conversions",
            "USD",
        );
        assert_eq!(thin.svg, None);
        assert!(thin.rows.is_empty());
        assert_eq!(
            thin.sentence.words(),
            "The age data is too thin: Google Ads knows the age for 79% of clicks."
        );

        let enough = demographic(
            title,
            "age group",
            "age",
            vec![group("25 to 34", true, 80.0), group("Unknown", false, 20.0)],
            "conversions",
            "USD",
        );
        assert_eq!(
            enough.sentence.words(),
            "25 to 34 takes 80% of spend and brings 80% of conversions."
        );
        assert_eq!(enough.rows.len(), 2);
        assert_eq!(
            enough.table.unwrap().rows[1],
            ["Unknown", "$40.00", "20%", "2", "20%"]
        );

        let none = demographic(title, "gender", "gender", vec![], "clicks", "USD");
        assert_eq!(
            none.sentence.words(),
            "Google Ads has no gender data for these 30 days."
        );
    }

    #[test]
    fn a_quality_score_is_a_bar_of_ten_parts() {
        assert_eq!(
            score_bar(crate::language::english(), 7).unwrap(),
            r#"<svg class="score" viewBox="0 0 118 8" width="118" height="8" role="img" aria-label="Quality score 7 of 10"><path class="score__track" d="M0 4h118"/><path class="score__value" d="M0 4h82"/></svg>"#
        );
        assert!(score_bar(crate::language::english(), 10)
            .unwrap()
            .contains(r#"d="M0 4h118"/></svg>"#));
        assert_eq!(
            meter(0.375, Some(1.2), "Conversion rate 3.0%. Campaign rate 8.0%"),
            r#"<svg class="meter" width="100%" height="8" role="img" aria-label="Conversion rate 3.0%. Campaign rate 8.0%"><rect class="meter__track" width="100%" height="8" rx="4"/><rect class="meter__value" width="37.50%" height="8" rx="4"/><rect class="meter__mark" x="100.00%" y="-3" width="2" height="14"/></svg>"#
        );
        assert!(!meter(1.0, None, "Spend $40.00").contains("meter__mark"));
        assert_eq!(
            quality_sentence(
                crate::language::english(),
                &[Some(3), Some(4), Some(8), None]
            )
            .unwrap(),
            "2 of the 3 keywords with the most spend have a Quality score of 4 or lower."
        );
        assert_eq!(
            quality_sentence(crate::language::english(), &[Some(4), Some(8)]).unwrap(),
            "1 of the 2 keywords with the most spend has a Quality score of 4 or lower."
        );
        assert_eq!(
            quality_sentence(crate::language::english(), &[Some(8)]).unwrap(),
            "The keyword with the most spend has a Quality score of 8."
        );
        assert_eq!(
            quality_sentence(crate::language::english(), &[None]).unwrap(),
            "Google Ads gives no Quality score for the keywords with the most spend."
        );
    }

    #[test]
    fn the_delay_sentence_needs_ten_conversions_and_a_late_one() {
        assert_eq!(
            delay_sentence(crate::language::english(), 200.0, 46.0)
                .unwrap()
                .as_deref(),
            Some("About 23% of conversions arrive more than 2 days after the ad was shown. The newest days are not complete.")
        );
        assert_eq!(
            delay_sentence(crate::language::english(), 9.0, 4.0).unwrap(),
            None
        );
        assert_eq!(
            delay_sentence(crate::language::english(), 200.0, 0.4).unwrap(),
            None
        );
    }
}
