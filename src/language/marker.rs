use std::{collections::BTreeSet, sync::OnceLock};

use super::{
    format::{CurrencyPlace, Format, Part, Range},
    text::placeholders,
    Language, Plural, Rule, Text, Written,
};

pub const LANGUAGE: Language = Language::Pt;

fn leak(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

fn marked(key: &str, form: &str, english: &str) -> &'static str {
    let mut names = Vec::new();
    for name in placeholders(english).unwrap_or_default() {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    let slots = names
        .iter()
        .map(|name| format!(" {{{name}}}"))
        .collect::<String>();
    leak(format!("‹{key}{form}›{slots}"))
}

pub(super) fn line(key: &str, english: &str) -> &'static str {
    marked(key, "", english)
}

pub(super) fn plural(key: &str, english: &Plural) -> Plural {
    Plural::three(
        marked(key, ".one", english.one),
        marked(key, ".few", english.few),
        marked(key, ".other", english.other),
    )
}

fn names(prefix: &str, count: usize) -> [&'static str; 12] {
    std::array::from_fn(|index| {
        if index < count {
            leak(format!("‹{prefix}{}›", index + 1))
        } else {
            ""
        }
    })
}

fn format() -> Format {
    use Part::{Day, Literal, Month, MonthShort, Weekday, WeekdayShort, Year};
    const SPACE: Part = Literal(" ");
    let months = names("month", 12);
    let months_short = names("mon", 12);
    let weekdays_all = names("day", 7);
    let weekdays_short_all = names("dow", 7);
    Format {
        locale: "ro",
        group: ".",
        decimal: ",",
        group_from_digits: 4,
        percent_suffix: " pct",
        usd_symbol: "US$",
        currency_place: CurrencyPlace::After,
        symbol_gap: " ",
        code_gap: " ",
        months,
        months_short,
        weekdays: std::array::from_fn(|index| weekdays_all[index]),
        weekdays_short: std::array::from_fn(|index| weekdays_short_all[index]),
        date_long: &[Month, SPACE, Day, Literal(", "), Weekday],
        date_day_month: &[Month, SPACE, Day],
        date_with_year: &[Year, Literal("/"), Month, Literal("/"), Day],
        date_short: &[MonthShort, SPACE, Day],
        date_weekday_short: &[WeekdayShort, Literal(", "), MonthShort, SPACE, Day],
        range_same_month: Range {
            start: &[Month, SPACE, Day],
            between: " ~ ",
            end: &[Day],
        },
        range_same_month_year: Range {
            start: &[Year, SPACE, Month, SPACE, Day],
            between: " ~ ",
            end: &[Day],
        },
        range_same_year: Range {
            start: &[Month, SPACE, Day],
            between: " ~ ",
            end: &[Month, SPACE, Day],
        },
        range_same_year_year: Range {
            start: &[Year, SPACE, Month, SPACE, Day],
            between: " ~ ",
            end: &[Month, SPACE, Day],
        },
        range_other_years: Range {
            start: &[Year, SPACE, Month, SPACE, Day],
            between: " ~ ",
            end: &[Year, SPACE, Month, SPACE, Day],
        },
    }
}

pub fn text() -> &'static Text {
    static TEXT: OnceLock<Text> = OnceLock::new();
    TEXT.get_or_init(|| {
        let english = super::english();
        Text::marked(
            english,
            Rule::Romanian,
            Written::marked(&english.written, format()),
        )
    })
}

pub fn english_words() -> BTreeSet<String> {
    let english = super::english();
    let mut templates = Vec::new();
    for key in Text::KEYS {
        match english.entry(key) {
            Some(super::Entry::Line(line)) => templates.push(line),
            Some(super::Entry::Plural(plural)) => {
                templates.extend([plural.one, plural.few, plural.other]);
            }
            None => {}
        }
    }
    for key in Written::KEYS {
        templates.extend(english.written.templates(key).unwrap_or_default());
    }
    let format = &english.written.format;
    templates.extend(format.months);
    templates.extend(format.months_short);
    templates.extend(format.weekdays);
    templates.extend(format.weekdays_short);
    let mut words = BTreeSet::new();
    for template in templates {
        let mut inside = false;
        let mut word = String::new();
        for c in template.chars().chain(std::iter::once(' ')) {
            match c {
                '{' => inside = true,
                '}' => inside = false,
                c if !inside && c.is_alphabetic() => word.extend(c.to_lowercase()),
                _ => {
                    if word.chars().count() >= 3 {
                        words.insert(std::mem::take(&mut word));
                    }
                    word.clear();
                }
            }
        }
    }
    words
}
