use chrono::NaiveDate;

use crate::language::{english, Text};

const OPEN: char = '⟦';
const CLOSE: char = '⟧';
const MARK_START: &str = "a";
const MARK_END: &str = "b";
const MAX_DIGITS: usize = 15;
const LEAP_YEAR: i32 = 2024;

#[derive(Clone, Debug, PartialEq)]
enum Value {
    Name(String),
    Money {
        amount: f64,
        decimals: usize,
        currency: String,
    },
    Percent {
        amount: f64,
        decimals: usize,
    },
    Number {
        amount: f64,
        decimals: usize,
    },
    Date {
        date: NaiveDate,
        year: bool,
    },
    Raw(String),
}

impl Value {
    fn write(&self, text: &Text) -> String {
        let format = &text.written.format;
        match self {
            Value::Name(name) | Value::Raw(name) => name.clone(),
            Value::Money {
                amount,
                decimals,
                currency,
            } => format.amount(*amount, *decimals, currency),
            Value::Percent { amount, decimals } => {
                format!(
                    "{}{}",
                    format.number(*amount, *decimals),
                    format.percent_suffix
                )
            }
            Value::Number { amount, decimals } => format.number(*amount, *decimals),
            Value::Date { date, year: true } => format.date_with_year(*date),
            Value::Date { date, year: false } => format.day_month(*date),
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct Masked {
    template: String,
    values: Vec<Value>,
    highlighted: bool,
}

#[derive(Debug, PartialEq)]
pub struct Restored {
    pub text: String,
    pub highlight: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Rejected(pub &'static str);

pub struct Account<'a> {
    pub names: &'a [String],
    pub currency: Option<&'a str>,
}

fn is_word(character: Option<char>) -> bool {
    character.is_some_and(char::is_alphanumeric)
}

struct ReadNumber {
    end: usize,
    text: String,
    decimals: usize,
    safe: bool,
}

struct Scan<'a> {
    text: &'a str,
    names: Vec<&'a str>,
    currency: Option<&'a str>,
    months: [&'static str; 12],
}

impl<'a> Scan<'a> {
    fn before(&self, at: usize) -> Option<char> {
        self.text[..at].chars().next_back()
    }

    fn after(&self, at: usize) -> Option<char> {
        self.text[at..].chars().next()
    }

    fn digits(&self, at: usize) -> usize {
        self.text[at..]
            .bytes()
            .take_while(u8::is_ascii_digit)
            .count()
    }

    fn name(&self, at: usize) -> Option<(usize, Value)> {
        if is_word(self.before(at)) {
            return None;
        }
        let rest = &self.text[at..];
        self.names
            .iter()
            .find(|name| rest.starts_with(**name) && !is_word(self.after(at + name.len())))
            .map(|name| (at + name.len(), Value::Name((*name).to_owned())))
    }

    fn date(&self, at: usize) -> Option<(usize, Value)> {
        let rest = &self.text[at..];
        let digits = self.digits(at);
        if digits == 4 && rest[4..].starts_with('-') && self.digits(at + 5) == 2 {
            if rest[7..].starts_with('-') && self.digits(at + 8) == 2 {
                let end = at + 10;
                let date = NaiveDate::parse_from_str(&rest[..10], "%Y-%m-%d").ok()?;
                return (!is_word(self.after(end)))
                    .then_some((end, Value::Date { date, year: true }));
            }
            return None;
        }
        if !(1..=2).contains(&digits) || !rest[digits..].starts_with(' ') {
            return None;
        }
        let day = rest[..digits].parse::<u32>().ok()?;
        let name = &rest[digits + 1..];
        let (month, length) = self
            .months
            .iter()
            .enumerate()
            .find(|(_, month)| name.starts_with(**month))
            .map(|(index, month)| (index as u32 + 1, month.len()))?;
        let mut end = at + digits + 1 + length;
        if self.after(end).is_some_and(char::is_alphabetic) {
            return None;
        }
        let mut year = None;
        if self.text[end..].starts_with(' ') && self.digits(end + 1) == 4 {
            let candidate = &self.text[end + 1..end + 5];
            if !is_word(self.after(end + 5))
                && (candidate.starts_with("19") || candidate.starts_with("20"))
            {
                year = candidate.parse::<i32>().ok();
                end += 5;
            }
        }
        let date = NaiveDate::from_ymd_opt(year.unwrap_or(LEAP_YEAR), month, day)?;
        Some((
            end,
            Value::Date {
                date,
                year: year.is_some(),
            },
        ))
    }

    fn number(&self, at: usize) -> Option<ReadNumber> {
        let mut end = at;
        let mut text = String::new();
        if self.text[end..].starts_with('-') {
            text.push('-');
            end += 1;
        }
        let first = self.digits(end);
        if first == 0 {
            return None;
        }
        text.push_str(&self.text[end..end + first]);
        end += first;
        let mut grouped = false;
        if first <= 3 {
            while self.text[end..].starts_with(',') && self.digits(end + 1) == 3 {
                text.push_str(&self.text[end + 1..end + 4]);
                end += 4;
                grouped = true;
            }
        }
        let mut decimals = 0;
        if self.text[end..].starts_with('.') && self.digits(end + 1) > 0 {
            decimals = self.digits(end + 1);
            text.push_str(&self.text[end..end + 1 + decimals]);
            end += 1 + decimals;
        }
        let digits = text.chars().filter(char::is_ascii_digit).count();
        let whole = text
            .trim_start_matches('-')
            .split('.')
            .next()
            .unwrap_or_default();
        let safe = digits <= MAX_DIGITS
            && !(whole.len() > 1 && whole.starts_with('0'))
            && (grouped || whole.len() < 4);
        Some(ReadNumber {
            end,
            text,
            decimals,
            safe,
        })
    }

    fn amount(&self, at: usize) -> Option<(usize, Value)> {
        let rest = &self.text[at..];
        let (currency, start) = if rest.starts_with('$') {
            ("USD".to_owned(), at + 1)
        } else {
            let code = self.currency?;
            if !rest.starts_with(code) || is_word(self.before(at)) {
                return None;
            }
            let gap = at + code.len();
            if !self.text[gap..].starts_with(' ') {
                return None;
            }
            (code.to_owned(), gap + 1)
        };
        let ReadNumber {
            end,
            text,
            decimals,
            safe,
        } = self.number(start)?;
        if !safe
            || self
                .after(end)
                .is_some_and(|c| c.is_alphabetic() || c == '%')
        {
            return None;
        }
        Some((
            end,
            Value::Money {
                amount: text.parse().ok()?,
                decimals,
                currency,
            },
        ))
    }

    fn plain_number(&self, at: usize) -> Option<(usize, Value)> {
        let signed = self.text[at..].starts_with('-');
        if signed
            && self
                .before(at)
                .is_some_and(|c| c.is_alphanumeric() || c == ')')
        {
            return None;
        }
        let ReadNumber {
            end,
            text,
            decimals,
            safe,
        } = self.number(at)?;
        let attached =
            |character: Option<char>| character.is_some_and(|c| c.is_alphabetic() || c == '_');
        if attached(self.after(end)) {
            let word = self.text[end..]
                .find(|c: char| !c.is_alphanumeric() && c != '_')
                .map_or(self.text.len(), |length| end + length);
            return Some((word, Value::Raw(self.text[at..word].to_owned())));
        }
        let percent = self.after(end) == Some('%');
        let end_with_sign = if percent { end + 1 } else { end };
        if !safe {
            return Some((
                end_with_sign,
                Value::Raw(self.text[at..end_with_sign].to_owned()),
            ));
        }
        let amount = text.parse().ok()?;
        Some((
            end_with_sign,
            if percent {
                Value::Percent { amount, decimals }
            } else {
                Value::Number { amount, decimals }
            },
        ))
    }

    fn word_with_digits(&self, at: usize) -> Option<(usize, Value)> {
        let first = self.after(at)?;
        if !first.is_alphabetic() || is_word(self.before(at)) {
            return None;
        }
        let length = self.text[at..]
            .find(|c: char| !c.is_alphanumeric() && c != '_')
            .unwrap_or(self.text.len() - at);
        let word = &self.text[at..at + length];
        word.chars()
            .any(|c| c.is_ascii_digit())
            .then(|| (at + length, Value::Raw(word.to_owned())))
    }
}

pub fn mask(text: &str, account: &Account<'_>, highlight: Option<&str>) -> Option<Masked> {
    if text.contains([OPEN, CLOSE]) {
        return None;
    }
    let mut names = account
        .names
        .iter()
        .map(String::as_str)
        .filter(|name| name.chars().count() >= 2 && !name.trim().is_empty())
        .collect::<Vec<_>>();
    names.sort_by(|a, b| b.len().cmp(&a.len()).then(a.cmp(b)));
    names.dedup();
    let scan = Scan {
        text,
        names,
        currency: account.currency,
        months: english().written.format.months,
    };
    let mut pieces: Vec<(usize, usize, Option<Value>)> = Vec::new();
    let mut at = 0;
    while at < text.len() {
        let found = scan
            .name(at)
            .or_else(|| scan.date(at))
            .or_else(|| scan.amount(at))
            .or_else(|| scan.plain_number(at))
            .or_else(|| scan.word_with_digits(at));
        match found {
            Some((end, value)) if end > at => {
                pieces.push((at, end, Some(value)));
                at = end;
            }
            _ => {
                let end = at + scan.after(at)?.len_utf8();
                pieces.push((at, end, None));
                at = end;
            }
        }
    }
    let span = highlight
        .and_then(|phrase| text.find(phrase).map(|start| (start, start + phrase.len())))
        .filter(|(start, end)| {
            let boundary = |at: usize| at == text.len() || pieces.iter().any(|piece| piece.0 == at);
            start < end && boundary(*start) && boundary(*end)
        });
    let mut template = String::new();
    let mut values: Vec<Value> = Vec::new();
    let mark =
        |template: &mut String, name: &str| template.push_str(&format!("{OPEN}{name}{CLOSE}"));
    for (start, end, value) in &pieces {
        if let Some((first, last)) = span {
            if *start == first {
                mark(&mut template, MARK_START);
            }
            if *start == last {
                mark(&mut template, MARK_END);
            }
        }
        match value {
            None => template.push_str(&text[*start..*end]),
            Some(value) => {
                let index = values
                    .iter()
                    .position(|known| known == value)
                    .unwrap_or_else(|| {
                        values.push(value.clone());
                        values.len() - 1
                    });
                template.push_str(&format!("{OPEN}{}{CLOSE}", index + 1));
            }
        }
    }
    if span.is_some_and(|(_, last)| last == text.len()) {
        mark(&mut template, MARK_END);
    }
    Some(Masked {
        template,
        values,
        highlighted: span.is_some(),
    })
}

fn placeholders(text: &str) -> Option<(Vec<&str>, String)> {
    let mut names = Vec::new();
    let mut rest = String::new();
    let mut tail = text;
    while let Some(start) = tail.find([OPEN, CLOSE]) {
        rest.push_str(&tail[..start]);
        if !tail[start..].starts_with(OPEN) {
            return None;
        }
        let inner = &tail[start + OPEN.len_utf8()..];
        let end = inner.find(CLOSE)?;
        let name = &inner[..end];
        if name.is_empty() || name.len() > 4 || !name.chars().all(|c| c.is_ascii_alphanumeric()) {
            return None;
        }
        names.push(name);
        tail = &inner[end + CLOSE.len_utf8()..];
    }
    rest.push_str(tail);
    Some((names, rest))
}

impl Masked {
    pub fn template(&self) -> &str {
        &self.template
    }

    pub fn has_values(&self) -> bool {
        !self.values.is_empty()
    }

    pub fn restore(&self, answer: &str, text: &Text) -> Result<Restored, Rejected> {
        let answer = answer.trim();
        if answer.is_empty() {
            return Err(Rejected("empty answer"));
        }
        if answer.contains(['<', '>']) {
            return Err(Rejected("markup in the answer"));
        }
        if answer.chars().any(char::is_control) {
            return Err(Rejected("control character in the answer"));
        }
        let (found, found_rest) = placeholders(answer).ok_or(Rejected("broken placeholder"))?;
        let (expected, expected_rest) =
            placeholders(&self.template).ok_or(Rejected("broken placeholder"))?;
        let (mut found_sorted, mut expected_sorted) = (found, expected);
        found_sorted.sort_unstable();
        expected_sorted.sort_unstable();
        if found_sorted != expected_sorted {
            return Err(Rejected("placeholders differ"));
        }
        let allowed = expected_rest
            .chars()
            .filter(|c| c.is_numeric())
            .collect::<Vec<_>>();
        if found_rest
            .chars()
            .any(|c| c.is_numeric() && !allowed.contains(&c))
        {
            return Err(Rejected("new number in the answer"));
        }
        if answer.chars().count() > 3 * self.template.chars().count() + 64 {
            return Err(Rejected("answer too long"));
        }
        let mut out = String::new();
        let mut highlight = None;
        let mut mark = None;
        let mut tail = answer;
        while let Some(start) = tail.find(OPEN) {
            out.push_str(&tail[..start]);
            let inner = &tail[start + OPEN.len_utf8()..];
            let end = inner.find(CLOSE).ok_or(Rejected("broken placeholder"))?;
            let name = &inner[..end];
            match name {
                MARK_START => mark = Some(out.len()),
                MARK_END => {
                    if let Some(from) = mark.take() {
                        if from < out.len() {
                            highlight = Some(out[from..].to_owned());
                        }
                    }
                }
                _ => {
                    let value = name
                        .parse::<usize>()
                        .ok()
                        .and_then(|number| number.checked_sub(1))
                        .and_then(|index| self.values.get(index))
                        .ok_or(Rejected("unknown placeholder"))?;
                    out.push_str(&value.write(text));
                }
            }
            tail = &inner[end + CLOSE.len_utf8()..];
        }
        out.push_str(tail);
        if out.trim().is_empty() {
            return Err(Rejected("empty answer"));
        }
        Ok(Restored {
            text: out,
            highlight: if self.highlighted { highlight } else { None },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::language::Language;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| (*name).to_owned()).collect()
    }

    fn masked(text: &str, list: &[&str]) -> Masked {
        let list = names(list);
        mask(
            text,
            &Account {
                names: &list,
                currency: Some("USD"),
            },
            None,
        )
        .unwrap()
    }

    fn romanian() -> &'static Text {
        Language::Ro.text().unwrap()
    }

    #[test]
    fn names_numbers_amounts_percents_and_dates_become_placeholders() {
        let masked = masked(
            "Search - Brand spent $1,234 against $980.50, up 18% from 88 on 11 September.",
            &["Search - Brand"],
        );
        assert_eq!(
            masked.template(),
            "⟦1⟧ spent ⟦2⟧ against ⟦3⟧, up ⟦4⟧ from ⟦5⟧ on ⟦6⟧."
        );
        assert!(masked.has_values());
    }

    #[test]
    fn values_are_written_again_with_the_formats_of_the_language() {
        let masked = masked(
            "Search - Brand spent $1,234 against $980.50, up 18% from 88 on 11 September.",
            &["Search - Brand"],
        );
        let restored = masked
            .restore(
                "⟦1⟧ a cheltuit ⟦2⟧ față de ⟦3⟧, în creștere cu ⟦4⟧ de la ⟦5⟧ pe ⟦6⟧.",
                romanian(),
            )
            .unwrap();
        assert_eq!(
            restored.text,
            "Search - Brand a cheltuit 1.234\u{a0}$ față de 980,50\u{a0}$, în creștere cu 18\u{a0}% de la 88 pe 11 septembrie."
        );
    }

    #[test]
    fn a_decimal_percent_keeps_its_decimals_and_a_euro_code_amount_follows_the_language() {
        let list = names(&[]);
        let account = Account {
            names: &list,
            currency: Some("EUR"),
        };
        let masked = mask(
            "Spend was EUR 12.50, a rise of 3.5% in 2026-10-11.",
            &account,
            None,
        )
        .unwrap();
        assert_eq!(masked.template(), "Spend was ⟦1⟧, a rise of ⟦2⟧ in ⟦3⟧.");
        let restored = masked
            .restore(
                "Cheltuiala a fost ⟦1⟧, o creștere de ⟦2⟧ în ⟦3⟧.",
                romanian(),
            )
            .unwrap();
        assert_eq!(
            restored.text,
            "Cheltuiala a fost 12,50\u{a0}EUR, o creștere de 3,5\u{a0}% în 11 octombrie 2026."
        );
    }

    #[test]
    fn a_name_with_regex_characters_is_masked_as_plain_text() {
        let masked = masked(
            "[Brand] (US) + .* spent more. Then [Brand] (US) + .* stopped.",
            &["[Brand] (US) + .*"],
        );
        assert_eq!(masked.template(), "⟦1⟧ spent more. Then ⟦1⟧ stopped.");
        let restored = masked
            .restore("⟦1⟧ a cheltuit mai mult. Apoi ⟦1⟧ s-a oprit.", romanian())
            .unwrap();
        assert_eq!(
            restored.text,
            "[Brand] (US) + .* a cheltuit mai mult. Apoi [Brand] (US) + .* s-a oprit."
        );
    }

    #[test]
    fn a_name_that_is_part_of_another_name_does_not_cut_it() {
        let masked = masked(
            "Brand Plus lost clicks while Brand held. Brands vary.",
            &["Brand", "Brand Plus"],
        );
        assert_eq!(
            masked.template(),
            "⟦1⟧ lost clicks while ⟦2⟧ held. Brands vary."
        );
        let restored = masked
            .restore(
                "⟦1⟧ a pierdut clicuri, iar ⟦2⟧ a rezistat. Brands variază.",
                romanian(),
            )
            .unwrap();
        assert!(restored.text.starts_with("Brand Plus a pierdut"));
        assert!(restored.text.contains("iar Brand a rezistat"));
    }

    #[test]
    fn digits_inside_a_name_are_part_of_the_name() {
        let masked = masked(
            "Search - Campaign 02 had 5 clicks; Search - Campaign 2 had 7.",
            &["Search - Campaign 02", "Search - Campaign 2"],
        );
        assert_eq!(masked.template(), "⟦1⟧ had ⟦2⟧ clicks; ⟦3⟧ had ⟦4⟧.");
        assert_eq!(
            masked
                .restore("⟦1⟧ a avut ⟦2⟧ clicuri; ⟦3⟧ a avut ⟦4⟧.", romanian())
                .unwrap()
                .text,
            "Search - Campaign 02 a avut 5 clicuri; Search - Campaign 2 a avut 7."
        );
    }

    #[test]
    fn a_repeated_value_is_one_placeholder_that_the_answer_must_repeat() {
        let masked = masked(
            "It had 30 clicks and 30 views of Brand and Brand.",
            &["Brand"],
        );
        assert_eq!(
            masked.template(),
            "It had ⟦1⟧ clicks and ⟦1⟧ views of ⟦2⟧ and ⟦2⟧."
        );
        assert_eq!(
            masked.restore(
                "A avut ⟦1⟧ clicuri și vizualizări ale ⟦2⟧ și ⟦2⟧.",
                romanian()
            ),
            Err(Rejected("placeholders differ"))
        );
        assert!(masked
            .restore(
                "A avut ⟦1⟧ clicuri și ⟦1⟧ vizualizări ale ⟦2⟧ și ⟦2⟧.",
                romanian()
            )
            .is_ok());
    }

    #[test]
    fn an_answer_with_a_missing_extra_or_changed_placeholder_is_rejected() {
        let masked = masked("Spend was $5 and clicks were 7.", &[]);
        let reject = |answer: &str| masked.restore(answer, romanian()).unwrap_err().0;
        assert_eq!(reject("Cheltuiala a fost ⟦1⟧."), "placeholders differ");
        assert_eq!(
            reject("Cheltuiala a fost ⟦1⟧ și ⟦2⟧ și ⟦3⟧."),
            "placeholders differ"
        );
        assert_eq!(
            reject("Cheltuiala a fost ⟦1⟧ și ⟦3⟧."),
            "placeholders differ"
        );
        assert_eq!(reject("Cheltuiala a fost ⟦1⟧ și ⟦2."), "broken placeholder");
        assert_eq!(
            reject("Cheltuiala a fost ⟦1⟧ și 7 ⟦2⟧."),
            "new number in the answer"
        );
        assert_eq!(
            reject("Cheltuiala a fost ⟦1⟧ și ⟦2⟧ <b>."),
            "markup in the answer"
        );
        assert_eq!(
            reject("Cheltuiala a fost ⟦1⟧ și ⟦2⟧ \u{0}."),
            "control character in the answer"
        );
        assert_eq!(reject("   "), "empty answer");
    }

    #[test]
    fn identifiers_and_years_are_restored_as_they_were() {
        let masked = masked(
            "Ad group A1 had 2026 clicks and ID 4829153307 and 007.",
            &[],
        );
        assert_eq!(
            masked.template(),
            "Ad group ⟦1⟧ had ⟦2⟧ clicks and ID ⟦3⟧ and ⟦4⟧."
        );
        assert_eq!(
            masked
                .restore(
                    "Grupul ⟦1⟧ a avut ⟦2⟧ clicuri și ID ⟦3⟧ și ⟦4⟧.",
                    romanian()
                )
                .unwrap()
                .text,
            "Grupul A1 a avut 2026 clicuri și ID 4829153307 și 007."
        );
    }

    #[test]
    fn a_list_of_numbers_is_not_read_as_one_grouped_number() {
        let masked = masked("Days 7,14,30 and 1,234,567 and 12,5.", &[]);
        assert_eq!(masked.template(), "Days ⟦1⟧,⟦2⟧,⟦3⟧ and ⟦4⟧ and ⟦5⟧,⟦6⟧.");
        assert_eq!(
            masked
                .restore("Zile ⟦1⟧,⟦2⟧,⟦3⟧ și ⟦4⟧ și ⟦5⟧,⟦6⟧.", romanian())
                .unwrap()
                .text,
            "Zile 7,14,30 și 1.234.567 și 12,5."
        );
    }

    #[test]
    fn a_text_with_the_bracket_of_a_placeholder_is_not_translated() {
        let list = names(&[]);
        let account = Account {
            names: &list,
            currency: None,
        };
        assert_eq!(mask("A ⟦1⟧ here", &account, None), None);
    }

    #[test]
    fn the_highlight_is_marked_and_comes_back() {
        let list = names(&[]);
        let account = Account {
            names: &list,
            currency: Some("USD"),
        };
        let masked = mask(
            "Conversions rose from 88 to 104 after the change. Spend held.",
            &account,
            Some("Conversions rose from 88 to 104"),
        )
        .unwrap();
        assert_eq!(
            masked.template(),
            "⟦a⟧Conversions rose from ⟦1⟧ to ⟦2⟧⟦b⟧ after the change. Spend held."
        );
        let restored = masked
            .restore(
                "⟦a⟧Conversiile au crescut de la ⟦1⟧ la ⟦2⟧⟦b⟧ după modificare. Cheltuiala a rămas stabilă.",
                romanian(),
            )
            .unwrap();
        assert_eq!(
            restored.text,
            "Conversiile au crescut de la 88 la 104 după modificare. Cheltuiala a rămas stabilă."
        );
        assert_eq!(
            restored.highlight.as_deref(),
            Some("Conversiile au crescut de la 88 la 104")
        );
        let swapped = masked
            .restore("⟦b⟧Conversiile ⟦1⟧ și ⟦2⟧⟦a⟧ după modificare.", romanian())
            .unwrap();
        assert_eq!(swapped.highlight, None);
    }

    #[test]
    fn a_highlight_that_cuts_a_value_is_not_marked() {
        let list = names(&[]);
        let account = Account {
            names: &list,
            currency: Some("USD"),
        };
        let masked = mask("Spend rose to 1,234 clicks.", &account, Some("rose to 1,2")).unwrap();
        assert_eq!(masked.template(), "Spend rose to ⟦1⟧ clicks.");
    }

    #[test]
    fn a_text_without_values_has_none() {
        assert!(!masked("Nothing notable changed.", &[]).has_values());
    }
}
