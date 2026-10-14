use chrono::{Datelike, NaiveDate};

#[derive(Clone, Copy, Debug)]
pub enum Part {
    Weekday,
    WeekdayShort,
    Day,
    Month,
    MonthShort,
    Year,
    Literal(&'static str),
}

pub type Pattern = &'static [Part];

#[derive(Clone, Copy, Debug)]
pub struct Range {
    pub start: Pattern,
    pub between: &'static str,
    pub end: Pattern,
}

#[derive(Clone, Copy, Debug)]
pub enum CurrencyPlace {
    Before,
    After,
}

pub struct Format {
    pub locale: &'static str,
    pub group: &'static str,
    pub decimal: &'static str,
    pub group_from_digits: usize,
    pub percent_suffix: &'static str,
    pub usd_symbol: &'static str,
    pub currency_place: CurrencyPlace,
    pub symbol_gap: &'static str,
    pub code_gap: &'static str,
    pub months: [&'static str; 12],
    pub months_short: [&'static str; 12],
    pub weekdays: [&'static str; 7],
    pub weekdays_short: [&'static str; 7],
    pub date_long: Pattern,
    pub date_day_month: Pattern,
    pub date_with_year: Pattern,
    pub date_short: Pattern,
    pub date_weekday_short: Pattern,
    pub range_same_month: Range,
    pub range_same_month_year: Range,
    pub range_same_year: Range,
    pub range_same_year_year: Range,
    pub range_other_years: Range,
}

impl Format {
    pub fn number(&self, n: f64, precision: usize) -> String {
        let rendered = format!("{n:.precision$}");
        let (whole, fraction) = rendered.split_once('.').unwrap_or((&rendered, ""));
        let mut result = String::new();
        let whole = if let Some(whole) = whole.strip_prefix('-') {
            result.push('-');
            whole
        } else {
            whole
        };
        let grouped = whole.len() >= self.group_from_digits;
        for (index, digit) in whole.chars().enumerate() {
            if grouped && index > 0 && (whole.len() - index).is_multiple_of(3) {
                result.push_str(self.group);
            }
            result.push(digit);
        }
        if !fraction.is_empty() {
            result.push_str(self.decimal);
            result.push_str(fraction);
        }
        result
    }

    pub fn money(&self, value: f64, currency: &str) -> String {
        self.with_currency(
            self.number(value, if value.abs() >= 100.0 { 0 } else { 2 }),
            currency,
        )
    }

    pub fn amount(&self, value: f64, decimals: usize, currency: &str) -> String {
        self.with_currency(self.number(value, decimals), currency)
    }

    pub fn whole_money(&self, value: f64, currency: &str) -> String {
        self.with_currency(self.number(value, 0), currency)
    }

    pub fn currency_mark<'a>(&'a self, currency: &'a str) -> &'a str {
        if currency == "USD" {
            self.usd_symbol
        } else {
            currency
        }
    }

    fn with_currency(&self, amount: String, currency: &str) -> String {
        let mark = self.currency_mark(currency);
        let gap = if currency == "USD" {
            self.symbol_gap
        } else {
            self.code_gap
        };
        match self.currency_place {
            CurrencyPlace::Before => format!("{mark}{gap}{amount}"),
            CurrencyPlace::After => format!("{amount}{gap}{mark}"),
        }
    }

    pub fn count(&self, value: f64) -> String {
        self.number(value, usize::from((value * 10.0).round() % 10.0 != 0.0))
    }

    fn write(&self, pattern: Pattern, date: NaiveDate) -> String {
        let mut out = String::new();
        for part in pattern {
            match part {
                Part::Weekday => {
                    out.push_str(self.weekdays[date.weekday().num_days_from_monday() as usize]);
                }
                Part::WeekdayShort => {
                    out.push_str(
                        self.weekdays_short[date.weekday().num_days_from_monday() as usize],
                    );
                }
                Part::Day => out.push_str(&date.day().to_string()),
                Part::Month => out.push_str(self.months[date.month0() as usize]),
                Part::MonthShort => out.push_str(self.months_short[date.month0() as usize]),
                Part::Year => out.push_str(&date.year().to_string()),
                Part::Literal(text) => out.push_str(text),
            }
        }
        out
    }

    fn write_range(&self, range: &Range, start: NaiveDate, end: NaiveDate) -> String {
        format!(
            "{}{}{}",
            self.write(range.start, start),
            range.between,
            self.write(range.end, end)
        )
    }

    pub fn long_date(&self, date: NaiveDate) -> String {
        self.write(self.date_long, date)
    }

    pub fn day_month(&self, date: NaiveDate) -> String {
        self.write(self.date_day_month, date)
    }

    pub fn weekday_name(&self, date: NaiveDate) -> &'static str {
        self.weekdays[date.weekday().num_days_from_monday() as usize]
    }

    pub fn date_with_year(&self, date: NaiveDate) -> String {
        self.write(self.date_with_year, date)
    }

    pub fn short_date(&self, date: NaiveDate) -> String {
        self.write(self.date_short, date)
    }

    pub fn weekday_short_date(&self, date: NaiveDate) -> String {
        self.write(self.date_weekday_short, date)
    }

    pub fn date_range(&self, start: NaiveDate, end: NaiveDate, with_year: bool) -> String {
        let range = if start.year() != end.year() {
            &self.range_other_years
        } else if start.month() == end.month() {
            if with_year {
                &self.range_same_month_year
            } else {
                &self.range_same_month
            }
        } else if with_year {
            &self.range_same_year_year
        } else {
            &self.range_same_year
        };
        self.write_range(range, start, end)
    }

    pub fn span(&self, start: NaiveDate, end: NaiveDate) -> String {
        let range = if start.month() == end.month() && start.year() == end.year() {
            &self.range_same_month
        } else {
            &self.range_same_year
        };
        self.write_range(range, start, end)
    }
}
