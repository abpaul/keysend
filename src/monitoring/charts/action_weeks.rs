use std::{collections::BTreeMap, fmt::Write as _};

use loco_rs::Result;

use crate::language::{say, say_plural, Text};

use super::{
    axis, chars, esc, nice_step, share, share_text, short, tip, width_class, Chart, Key, Sentence,
    Table, BAR_INSET, BAR_WIDTH, CHAR, SLOT, WEEKS, WEEKS_HEIGHT, WEEKS_NARROW, WEEKS_WIDE,
};

pub const ACTIONS_SHOWN: usize = 4;
const KINDS: [&str; ACTIONS_SHOWN + 1] = ["act-0", "act-1", "act-2", "act-3", "act-rest"];

pub fn action_weeks(
    text: &Text,
    starts: &[i64],
    conversions: &[(i64, String, f64)],
) -> Result<Chart> {
    let title = say_plural!(text, action_weeks_title, starts.len().max(1) as u64)?;
    let mut totals: BTreeMap<&str, f64> = BTreeMap::new();
    for (_, action, value) in conversions {
        *totals.entry(action.as_str()).or_default() += value;
    }
    let mut ranked = totals
        .into_iter()
        .filter(|(_, total)| *total > 0.0)
        .collect::<Vec<_>>();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(b.0)));
    let all = ranked.iter().map(|(_, total)| total).sum::<f64>();
    if starts.is_empty() || all <= 0.0 {
        return Ok(Chart::empty(
            say_plural!(text, action_weeks_title, WEEKS as u64)?,
            say_plural!(text, action_weeks_empty, WEEKS as u64)?,
        ));
    }
    let mut names = ranked
        .iter()
        .take(ACTIONS_SHOWN)
        .map(|(name, _)| (*name).to_owned())
        .collect::<Vec<_>>();
    let folded = ranked.len() > ACTIONS_SHOWN;
    if folded {
        names.push(text.written.action_weeks_other.into());
    }
    let kind = |index: usize| {
        if folded && index == ACTIONS_SHOWN {
            KINDS[ACTIONS_SHOWN]
        } else {
            KINDS[index]
        }
    };
    let mut weeks = vec![vec![0.0; names.len()]; starts.len()];
    for (week, action, value) in conversions {
        let Some(values) = usize::try_from(*week)
            .ok()
            .and_then(|week| weeks.get_mut(week))
        else {
            continue;
        };
        let part = names
            .iter()
            .position(|name| name == action)
            .unwrap_or(names.len() - 1);
        values[part] += value;
    }
    let sums = weeks
        .iter()
        .map(|values| values.iter().sum::<f64>())
        .collect::<Vec<_>>();
    let step = nice_step(sums.iter().copied().fold(0.0, f64::max) * 1.15 / 2.0);
    let y = |value: f64| WEEKS_HEIGHT - value / (2.0 * step) * WEEKS_HEIGHT;
    let newest = weeks.len() - 1;
    let slot = |index: usize| (WEEKS as usize - weeks.len() + index) as f64 * SLOT;
    let center = |index: usize| slot(index) + BAR_INSET + BAR_WIDTH / 2.0;

    let mut svg = format!(
        r#"<svg class="chart__svg" height="170" role="group" aria-label="{}">"#,
        esc(&title)
    );
    for tick in 1..=2 {
        let value = step * f64::from(tick);
        let _ = write!(
            svg,
            r#"<line class="grid" x1="0" x2="100%" y1="{0:.1}" y2="{0:.1}"/><text x="-30" y="{1:.1}">{2}</text>"#,
            y(value),
            y(value) + 4.0,
            esc(&axis(text, text.count(value)))
        );
    }
    let widest = sums
        .iter()
        .map(|sum| chars(&text.count(*sum)))
        .fold(0.0, f64::max)
        * CHAR;
    let stride = |plot: f64| ((widest + 4.0) / (plot / WEEKS as f64)).ceil().max(1.0) as usize;
    let mut labels = String::new();
    let mut rows = Vec::with_capacity(weeks.len());
    for (index, values) in weeks.iter().enumerate() {
        let from = short(text, starts[index])?;
        let open = index == newest;
        let total = text.count(sums[index]);
        let mut lines = vec![if open {
            say!(text, action_line_open, from = &from, conversions = &total)?
        } else {
            say!(text, action_line, from = &from, conversions = &total)?
        }];
        let mut bars = String::new();
        let mut below = 0.0;
        for (part, value) in values.iter().enumerate().filter(|(_, value)| **value > 0.0) {
            let top = y(below + value);
            let _ = write!(
                bars,
                r#"<rect class="seg seg--{}" x="{:.2}%" y="{top:.1}" width="{BAR_WIDTH:.2}%" height="{:.1}"/>"#,
                kind(part),
                slot(index) + BAR_INSET,
                y(below) - top
            );
            below += value;
            lines.push(say!(
                text,
                action_part,
                name = &names[part],
                value = text.count(*value)
            )?);
        }
        let behind = newest - index;
        let shown = |plot: f64| sums[index] > 0.0 && behind.is_multiple_of(stride(plot));
        if let Some(class) = width_class(shown(WEEKS_WIDE), shown(WEEKS_NARROW)) {
            let _ = write!(
                labels,
                r#"<text class="chart__value{class}" x="{:.2}%" y="{:.1}" text-anchor="middle">{}</text>"#,
                center(index),
                y(sums[index]) - 5.0,
                esc(&total)
            );
        }
        let to_left = center(index) > 50.0;
        let _ = write!(
            svg,
            r#"<g class="pt" tabindex="0" role="img"><title>{}</title><rect class="pt__hit" x="{:.2}%" y="0" width="{SLOT:.2}%" height="{WEEKS_HEIGHT:.0}"/>{bars}<svg x="{:.2}%" width="1" height="1" overflow="visible">{}</svg></g>"#,
            esc(&format!("{}.", lines.join(". "))),
            slot(index),
            center(index),
            tip(if to_left { -24.0 } else { 24.0 }, 0.0, &lines, to_left),
        );
        let mut row = vec![if open {
            say!(text, weeks_row_open, from = &from)?
        } else {
            say!(text, weeks_row, from = &from)?
        }];
        row.extend(values.iter().map(|value| text.count(*value)));
        row.push(total);
        rows.push(row);
    }
    svg.push_str(&labels);
    let _ = write!(
        svg,
        r#"<text x="{:.2}%" y="{:.0}">{}</text>"#,
        slot(0) + BAR_INSET,
        WEEKS_HEIGHT + 18.0,
        esc(&short(text, starts[0])?)
    );
    if newest > 1 {
        let _ = write!(
            svg,
            r#"<text x="100%" dx="-8" y="{:.0}" text-anchor="end">{}</text>"#,
            WEEKS_HEIGHT + 18.0,
            esc(&say!(
                text,
                weeks_axis_open,
                date = short(text, starts[newest])?
            )?)
        );
    }
    svg.push_str("</svg>");

    let mut headers = vec![text.written.weeks_header_week.to_owned()];
    headers.extend(names.iter().cloned());
    headers.push(text.written.action_header_all.into());
    Ok(Chart {
        sentence: Sentence {
            strong: ranked[0].0.to_owned(),
            text: text.written.action_has.into(),
            mark: say_plural!(
                text,
                action_mark,
                weeks.len() as u64,
                share = share_text(text, share(ranked[0].1, all))
            )?,
            after: format!(" {}", text.written.weeks_open),
        },
        svg: Some(svg),
        rows: Vec::new(),
        keys: names
            .iter()
            .enumerate()
            .map(|(index, name)| Key {
                kind: kind(index),
                label: name.clone(),
            })
            .collect(),
        table: Some(Table {
            caption: text.written.action_caption.into(),
            headers,
            rows,
            ..Default::default()
        }),
        title,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const END: i64 = 20_737;

    fn starts(weeks: i64) -> Vec<i64> {
        (0..weeks).rev().map(|week| END - week * 7 - 6).collect()
    }

    #[test]
    fn each_week_is_one_bar_stacked_by_conversion_action() {
        let mut rows = Vec::new();
        for week in 0..12 {
            rows.push((week, "Booking form".to_owned(), 12.0));
            rows.push((week, "Phone call".to_owned(), 6.0));
        }
        rows.push((11, "Chat".to_owned(), 2.0));
        let chart = action_weeks(crate::language::english(), &starts(12), &rows).unwrap();
        assert_eq!(chart.title, "Conversions by conversion action, 12 weeks");
        assert_eq!(
            chart.sentence.words(),
            "Booking form has 66% of the conversions of the last 12 weeks. The last week is not complete: conversions can still arrive."
        );
        let svg = chart.svg.unwrap();
        assert!(svg.contains(
            r#"<line class="grid" x1="0" x2="100%" y1="65.0" y2="65.0"/><text x="-30" y="69.0">15</text>"#
        ), "{svg}");
        assert!(svg.contains(
            r#"<line class="grid" x1="0" x2="100%" y1="0.0" y2="0.0"/><text x="-30" y="4.0">30</text>"#
        ));
        assert!(svg.contains(
            r#"<rect class="seg seg--act-0" x="0.66%" y="78.0" width="7.00%" height="52.0"/><rect class="seg seg--act-1" x="0.66%" y="52.0" width="7.00%" height="26.0"/>"#
        ));
        assert!(svg.contains(
            r#"<rect class="seg seg--act-2" x="92.33%" y="43.3" width="7.00%" height="8.7"/>"#
        ));
        assert!(svg.contains(
            r#"<text class="chart__value" x="95.83%" y="38.3" text-anchor="middle">20</text>"#
        ));
        assert!(svg.contains(
            "<title>Week of 5 Oct (so far): 20 conversions. Booking form: 12. Phone call: 6. Chat: 2.</title>"
        ));
        assert_eq!(svg.matches(r#"class="pt" tabindex="0""#).count(), 12);
        assert!(svg.contains(r#"<text x="0.66%" y="148">20 Jul</text>"#));
        assert!(svg
            .contains(r#"<text x="100%" dx="-8" y="148" text-anchor="end">5 Oct (so far)</text>"#));
        let table = chart.table.unwrap();
        assert_eq!(
            table.headers,
            [
                "Week",
                "Booking form",
                "Phone call",
                "Chat",
                "All conversions"
            ]
        );
        assert_eq!(table.rows[0], ["Week of 20 Jul", "12", "6", "0", "18"]);
        assert_eq!(
            table.rows[11],
            ["Week of 5 Oct (so far)", "12", "6", "2", "20"]
        );
        assert_eq!(
            chart
                .keys
                .iter()
                .map(|key| (key.kind, key.label.as_str()))
                .collect::<Vec<_>>(),
            [
                ("act-0", "Booking form"),
                ("act-1", "Phone call"),
                ("act-2", "Chat")
            ]
        );
    }

    #[test]
    fn more_than_four_conversion_actions_fold_into_one_part_and_no_conversions_is_one_sentence() {
        let rows = ["A", "B", "C", "D", "E", "F"]
            .iter()
            .enumerate()
            .map(|(index, name)| (0, (*name).to_owned(), 10.0 - index as f64))
            .collect::<Vec<_>>();
        let chart = action_weeks(crate::language::english(), &starts(3), &rows).unwrap();
        assert_eq!(chart.title, "Conversions by conversion action, 3 weeks");
        assert_eq!(
            chart
                .keys
                .iter()
                .map(|key| (key.kind, key.label.as_str()))
                .collect::<Vec<_>>(),
            [
                ("act-0", "A"),
                ("act-1", "B"),
                ("act-2", "C"),
                ("act-3", "D"),
                ("act-rest", "Other conversion actions")
            ]
        );
        assert_eq!(
            chart.table.unwrap().rows[0],
            ["Week of 21 Sep", "10", "9", "8", "7", "11", "45"]
        );

        let none = action_weeks(crate::language::english(), &starts(12), &[]).unwrap();
        assert_eq!(none.svg, None);
        assert_eq!(
            none.sentence.words(),
            "No conversion action had conversions in the last 12 weeks."
        );
    }
}
