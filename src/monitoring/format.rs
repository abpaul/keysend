use chrono::NaiveDate;
use loco_rs::Result;
use serde::Serialize;

use super::metrics::{Metrics, WindowMetrics};
use crate::language::{Text, Written};

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Cell {
    pub label: &'static str,
    pub value: String,
    pub change: String,
}

pub fn long_date(text: &Text, date: NaiveDate) -> String {
    text.written.format.long_date(date)
}

pub fn date_range(text: &Text, start: NaiveDate, end: NaiveDate, with_year: bool) -> String {
    text.written.format.date_range(start, end, with_year)
}

fn moved(text: &Text, current: f64, previous: f64, show: impl Fn(f64) -> String) -> Result<String> {
    text.change_from(current, previous, &show(previous))
}

pub fn ratio(amount: f64, by: Option<f64>) -> Option<f64> {
    by.filter(|by| *by > 0.0).map(|by| amount / by)
}

pub struct Column {
    pub label: fn(&Written) -> &'static str,
    pub missing: fn(&Written) -> &'static str,
    pub read: fn(&Metrics) -> Option<f64>,
    pub money: bool,
}

impl Column {
    pub fn show(&self, text: &Text, value: f64, currency: &str) -> String {
        if self.money {
            text.money(value, currency)
        } else {
            text.count(value)
        }
    }
}

pub const SPEND: Column = Column {
    label: |w| w.metric_spend,
    missing: |w| w.missing_spend,
    read: |m| Some(m.spend),
    money: true,
};
pub const CONVERSIONS: Column = Column {
    label: |w| w.metric_conversions,
    missing: |w| w.missing_conversions,
    read: |m| m.conversions,
    money: false,
};
pub const COST_PER_CONVERSION: Column = Column {
    label: |w| w.metric_cost_per_conversion,
    missing: |w| w.missing_conversions,
    read: |m| ratio(m.spend, m.conversions),
    money: true,
};
pub const CLICKS: Column = Column {
    label: |w| w.metric_clicks,
    missing: |w| w.missing_clicks,
    read: |m| Some(m.clicks),
    money: false,
};
pub const COST_PER_CLICK: Column = Column {
    label: |w| w.metric_cost_per_click,
    missing: |w| w.missing_clicks,
    read: |m| ratio(m.spend, Some(m.clicks)),
    money: true,
};
pub const IMPRESSIONS: Column = Column {
    label: |w| w.metric_impressions,
    missing: |w| w.missing_impressions,
    read: |m| Some(m.impressions),
    money: false,
};

fn cell(text: &Text, column: &Column, window: &WindowMetrics, currency: &str) -> Result<Cell> {
    let show = |value: f64| column.show(text, value, currency);
    let label = (column.label)(&text.written);
    Ok(
        match (
            (column.read)(&window.current),
            (column.read)(&window.previous),
        ) {
            (Some(current), Some(previous)) => Cell {
                label,
                value: show(current),
                change: moved(text, current, previous, show)?,
            },
            (Some(current), None) => Cell {
                label,
                value: show(current),
                change: text.written.no_percentage_comparison.into(),
            },
            (None, _) => Cell {
                label,
                value: "–".into(),
                change: (column.missing)(&text.written).into(),
            },
        },
    )
}

pub fn cells(text: &Text, window: &WindowMetrics, currency: &str) -> Result<Vec<Cell>> {
    let columns: &[&Column] = if window.current.conversions.is_some() {
        &[
            &SPEND,
            &CONVERSIONS,
            &COST_PER_CONVERSION,
            &CLICKS,
            &COST_PER_CLICK,
            &IMPRESSIONS,
        ]
    } else {
        &[&SPEND, &CLICKS, &COST_PER_CLICK, &IMPRESSIONS]
    };
    columns
        .iter()
        .map(|column| cell(text, column, window, currency))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::language::english;
    use crate::monitoring::metrics::Window;

    fn window(current: Metrics, previous: Metrics) -> WindowMetrics {
        WindowMetrics {
            window: Window::SevenDays,
            end_day: 0,
            current,
            previous,
        }
    }

    fn metrics(spend: f64, clicks: f64, impressions: f64, conversions: Option<f64>) -> Metrics {
        Metrics {
            spend,
            clicks,
            impressions,
            conversions,
            conversion_value: None,
        }
    }

    #[test]
    fn a_judged_window_leads_with_conversions_and_their_cost() {
        let cells = cells(
            english(),
            &window(
                metrics(4120.0, 1200.0, 30000.0, Some(104.0)),
                metrics(4090.0, 1100.0, 29000.0, Some(88.0)),
            ),
            "USD",
        )
        .unwrap();
        let labels: Vec<_> = cells.iter().map(|cell| cell.label).collect();
        assert_eq!(
            labels,
            [
                "Spend",
                "Conversions",
                "Cost per conversion",
                "Clicks",
                "Cost per click",
                "Impressions"
            ]
        );
        assert_eq!(cells[0].value, "$4,120");
        assert_eq!(cells[0].change, "up 0.7% from $4,090");
        assert_eq!(cells[1].value, "104");
        assert_eq!(cells[1].change, "up 18% from 88");
        assert_eq!(cells[2].value, "$39.62");
        assert_eq!(cells[2].change, "down 15% from $46.48");
    }

    #[test]
    fn a_window_without_conversions_has_no_conversion_cell() {
        let cells = cells(
            english(),
            &window(
                metrics(70.0, 70.0, 700.0, None),
                metrics(70.0, 70.0, 700.0, None),
            ),
            "EUR",
        )
        .unwrap();
        let labels: Vec<_> = cells.iter().map(|cell| cell.label).collect();
        assert_eq!(labels, ["Spend", "Clicks", "Cost per click", "Impressions"]);
        assert_eq!(cells[0].change, "no change");
    }

    #[test]
    fn a_cost_without_a_divisor_is_a_dash() {
        let cells = cells(
            english(),
            &window(
                metrics(50.0, 5.0, 50.0, Some(0.0)),
                metrics(50.0, 5.0, 50.0, Some(2.0)),
            ),
            "USD",
        )
        .unwrap();
        assert_eq!(cells[2].value, "–");
        assert_eq!(cells[2].change, "no conversions");
    }

    #[test]
    fn dates_read_as_a_person_writes_them() {
        let day = |month, day| NaiveDate::from_ymd_opt(2026, month, day).unwrap();
        assert_eq!(long_date(english(), day(10, 12)), "Monday 12 October");
        assert_eq!(
            date_range(english(), day(10, 5), day(10, 11), false),
            "5 to 11 October"
        );
        assert_eq!(
            date_range(english(), day(9, 28), day(10, 4), true),
            "28 September to 4 October 2026"
        );
        assert_eq!(
            date_range(
                english(),
                NaiveDate::from_ymd_opt(2025, 12, 30).unwrap(),
                day(1, 5),
                true
            ),
            "30 December 2025 to 5 January 2026"
        );
    }
}
