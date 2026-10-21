use crate::{
    settings::Settings,
    workspace::{
        measurement::{Operation, Usage},
        WorkspaceStore,
    },
};
use serde_json::Value;
use std::collections::BTreeSet;

const CAUSES: [&str; 6] = [
    "because",
    "due to",
    "caused by",
    "thanks to",
    "as a result of",
    "owing to",
];
const MAGNITUDES: [&str; 5] = ["hundred", "thousand", "million", "billion", "dozen"];
const TERMS: &str = "smart bidding target cpa maximize value broad phrase exact match negative keyword keywords responsive ad rsa demand gen impression share budget landing page pages mobile desktop tablet computer bid bids strategy campaign campaigns group groups assets asset sitelinks callouts audience audiences remarketing conversions conversion clicks impressions spend cost google ads metric metrics cpc ctr roas search display performance max shopping video quality score monday tuesday wednesday thursday friday saturday sunday january february march april may june july august september october november december i";

pub(crate) fn numbers(text: &str) -> BTreeSet<String> {
    let mut result = BTreeSet::new();
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '-'
            && characters
                .peek()
                .is_some_and(|character| matches!(character, '$' | '€' | '£' | '¥'))
        {
            characters.next();
        }
        if character.is_ascii_digit()
            || (character == '-' && characters.peek().is_some_and(char::is_ascii_digit))
        {
            let mut value = String::from(character);
            while characters.peek().is_some_and(|character| {
                character.is_ascii_digit() || matches!(character, '.' | ',')
            }) {
                value.push(characters.next().unwrap_or_default());
            }
            let value = value.trim_end_matches(['.', ',']).replace(',', "");
            let value = if value.contains('.') {
                value.trim_end_matches('0').trim_end_matches('.').to_owned()
            } else {
                value
            };
            result.insert(if value == "-0" { "0".into() } else { value });
        }
    }
    result
}

fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
}

pub(crate) struct Known {
    numbers: BTreeSet<String>,
    percents: BTreeSet<String>,
    moneys: BTreeSet<String>,
    words: BTreeSet<String>,
}

pub(crate) fn percents(text: &str) -> BTreeSet<String> {
    let mut result = BTreeSet::new();
    let characters = text.char_indices().collect::<Vec<_>>();
    for (position, (index, character)) in characters.iter().enumerate() {
        if *character != '%' {
            continue;
        }
        let digits = characters[..position]
            .iter()
            .rposition(|(_, character)| !character.is_whitespace())
            .map_or(0, |last| last + 1);
        let start = characters[..digits]
            .iter()
            .rev()
            .take_while(|(_, character)| {
                character.is_ascii_digit() || matches!(character, '.' | ',')
            })
            .last()
            .map_or(*index, |(start, _)| *start);
        result.extend(numbers(&text[start..*index]));
    }
    result
}

pub(crate) fn moneys(text: &str) -> BTreeSet<String> {
    let mut result = BTreeSet::new();
    let characters = text.char_indices().collect::<Vec<_>>();
    for (position, (index, character)) in characters.iter().enumerate() {
        if !character.is_ascii_digit()
            || position.checked_sub(1).is_some_and(|before| {
                characters[before].1.is_ascii_digit() || matches!(characters[before].1, '.' | ',')
            })
        {
            continue;
        }
        let before = characters[..position]
            .iter()
            .rev()
            .map(|(_, character)| *character)
            .skip_while(|character| *character == ' ')
            .take(3)
            .collect::<Vec<_>>();
        let sign = before
            .first()
            .is_some_and(|character| matches!(character, '$' | '€' | '£' | '¥'));
        let code = before.len() == 3 && before.iter().all(char::is_ascii_uppercase);
        if sign || code {
            let end = characters[position..]
                .iter()
                .find(|(_, character)| {
                    !(character.is_ascii_digit() || matches!(character, '.' | ','))
                })
                .map_or(text.len(), |(end, _)| *end);
            result.extend(numbers(&text[*index..end]));
        }
    }
    result
}

impl Known {
    pub(crate) fn of(values: &[&Value]) -> Self {
        let text = values
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        let lower = text.to_lowercase();
        Self {
            numbers: numbers(&text),
            percents: percents(&text),
            moneys: moneys(&text),
            words: words(&lower)
                .map(str::to_owned)
                .chain(TERMS.split(' ').map(str::to_owned))
                .collect(),
        }
    }
}

pub(crate) fn valid(text: &str, known: &[&Known], length: usize) -> Result<(), &'static str> {
    check(text, known, length, false)
}

fn check(
    text: &str,
    known: &[&Known],
    length: usize,
    causal_words: bool,
) -> Result<(), &'static str> {
    if text.trim().is_empty() {
        return Err("empty text");
    }
    if text.chars().count() > length {
        return Err("too long");
    }
    if text.contains(['\u{2014}', '\u{2013}', '\u{00b7}', '"']) {
        return Err("forbidden punctuation");
    }
    if text
        .chars()
        .any(|character| character.is_numeric() && !character.is_ascii_digit())
    {
        return Err("number that is not written in ASCII digits");
    }
    let lower = text.to_lowercase();
    if !causal_words && CAUSES.iter().any(|cause| lower.contains(cause)) {
        return Err("claimed cause");
    }
    if words(&lower).any(|word| {
        MAGNITUDES.contains(&word.trim_end_matches('s'))
            && !known.iter().any(|known| known.words.contains(word))
    }) {
        return Err("number written in words");
    }
    let unknown = numbers(text)
        .into_iter()
        .filter(|number| !known.iter().any(|known| known.numbers.contains(number)))
        .collect::<Vec<_>>();
    if !unknown.is_empty() {
        tracing::warn!(
            rejected = %unknown.join(" "),
            "Numbers that are not in the data"
        );
        return Err("number that is not in the data");
    }
    if percents(text)
        .iter()
        .any(|percent| !known.iter().any(|known| known.percents.contains(percent)))
    {
        return Err("percentage that is not in the data");
    }
    if moneys(text)
        .iter()
        .any(|money| !known.iter().any(|known| known.moneys.contains(money)))
    {
        return Err("amount of money that is not in the data");
    }
    let mut sentence_start = true;
    for token in text.split_whitespace() {
        for word in words(token) {
            if !sentence_start
                && word.chars().next().is_some_and(char::is_uppercase)
                && !known
                    .iter()
                    .any(|known| known.words.contains(word.to_lowercase().as_str()))
            {
                return Err("name that is not in the data");
            }
            sentence_start = false;
        }
        sentence_start = token.ends_with(['.', '!', '?', ':', ';']);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Claim {
    Nothing,
    Confirmed,
    Likely,
}

/// What a text may state about its cause. A causal word is allowed only in a sentence that
/// holds one of the anchors of the chosen cause, and a likely cause also needs its likelihood
/// word in that sentence. `names` are removed from the text first, so that a Campaign name
/// cannot supply a word.
pub(crate) struct Stated<'a> {
    pub claim: Claim,
    pub anchors: &'a [String],
    pub names: &'a [&'a str],
}

pub(crate) const NOTHING: Stated<'static> = Stated {
    claim: Claim::Nothing,
    anchors: &[],
    names: &[],
};

const OTHER_CAUSES: [&str; 2] = ["driven by", "led to"];

fn without_names(text: &str, names: &[&str]) -> String {
    let mut lower = text.to_lowercase();
    let mut names = names
        .iter()
        .map(|name| name.to_lowercase())
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>();
    names.sort_by_key(|name| std::cmp::Reverse(name.len()));
    for name in names {
        lower = lower.replace(&name, " ");
    }
    lower
}

fn sentences(text: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut start = 0;
    let mut characters = text.char_indices().peekable();
    while let Some((index, character)) = characters.next() {
        let ends = match character {
            ';' => true,
            '.' | '!' | '?' => characters
                .peek()
                .is_none_or(|(_, next)| next.is_whitespace()),
            _ => false,
        };
        if ends {
            found.push(&text[start..index]);
            start = index + character.len_utf8();
        }
    }
    found.push(&text[start..]);
    found
}

fn holds_phrase(sentence: &str, phrase: &str) -> bool {
    let phrase = phrase.to_lowercase();
    sentence.match_indices(&phrase).any(|(start, found)| {
        let end = start + found.len();
        !sentence[..start]
            .chars()
            .next_back()
            .is_some_and(char::is_alphanumeric)
            && !sentence[end..]
                .chars()
                .next()
                .is_some_and(char::is_alphanumeric)
    })
}

fn causal(sentence: &str) -> bool {
    CAUSES
        .iter()
        .chain(&OTHER_CAUSES)
        .any(|cause| sentence.contains(cause))
}

pub(crate) fn valid_monitoring(
    text: &str,
    known: &[&Known],
    length: usize,
    stated: &Stated,
) -> Result<(), &'static str> {
    check(text, known, length, stated.claim != Claim::Nothing)?;
    let lower = without_names(text, stated.names);
    let holds_anchor = |sentence: &str| {
        stated
            .anchors
            .iter()
            .any(|anchor| holds_phrase(sentence, anchor))
    };
    match stated.claim {
        Claim::Nothing => {
            if causal(&lower) {
                return Err("claimed cause");
            }
        }
        Claim::Confirmed | Claim::Likely => {
            let sentences = sentences(&lower);
            if !sentences.iter().any(|sentence| holds_anchor(sentence)) {
                return Err("cause not stated");
            }
            for sentence in sentences.into_iter().filter(|sentence| causal(sentence)) {
                if !holds_anchor(sentence) {
                    return Err("causal word without the cause");
                }
                if stated.claim == Claim::Likely
                    && !(sentence.contains("most likely")
                        || words(sentence).any(|word| word == "probably"))
                {
                    return Err("likely cause not marked as likely");
                }
            }
        }
    }
    Ok(())
}

const HEDGES: [&str; 9] = [
    "may",
    "might",
    "could",
    "possibly",
    "probably",
    "likely",
    "perhaps",
    "worth checking",
    "unclear whether",
];

fn hedged(sentence: &str) -> bool {
    HEDGES.iter().any(|hedge| holds_phrase(sentence, hedge))
}

/// A text that may give a reason only as a possibility: every sentence with a causal word also
/// holds a hedge such as may or probably, and with `explanation` the text holds one at least.
pub(crate) fn valid_hedged(
    text: &str,
    known: &[&Known],
    length: usize,
    names: &[&str],
    explanation: bool,
) -> Result<(), &'static str> {
    check(text, known, length, true)?;
    let lower = without_names(text, names);
    if sentences(&lower)
        .into_iter()
        .any(|sentence| causal(sentence) && !hedged(sentence))
    {
        return Err("cause stated as a fact");
    }
    if explanation && !hedged(&lower) {
        return Err("explanation not marked as possible");
    }
    Ok(())
}

pub(crate) async fn request(
    settings: &Settings,
    store: Option<&WorkspaceStore<'_>>,
    key: &str,
    payload: &Value,
) -> Option<Value> {
    request_with_sources(settings, store, key, payload)
        .await
        .map(|(answer, _)| answer)
}

/// The answer and the URLs that a web search of the request returned or cited.
pub(crate) async fn request_with_sources(
    settings: &Settings,
    store: Option<&WorkspaceStore<'_>>,
    key: &str,
    payload: &Value,
) -> Option<(Value, BTreeSet<String>)> {
    request_measured(settings, store, key, payload).await.0
}

/// The answer with the tokens that the request used, for a caller that keeps its own budget.
pub(crate) async fn request_measured(
    settings: &Settings,
    store: Option<&WorkspaceStore<'_>>,
    key: &str,
    payload: &Value,
) -> (Option<(Value, BTreeSet<String>)>, Usage) {
    let started = std::time::Instant::now();
    let mut usage = Usage::default();
    let outcome = read_response(settings, key, payload, &mut usage).await;
    if let Some(store) = store {
        if let Some(op) = payload
            .pointer("/text/format/name")
            .and_then(Value::as_str)
            .and_then(Operation::from_schema)
        {
            store
                .record_operation(
                    op,
                    payload["model"].as_str().unwrap_or_default(),
                    outcome.is_some(),
                    started.elapsed(),
                    usage,
                )
                .await;
        } else {
            tracing::warn!("AI request has no measurement kind");
        }
    }
    (outcome, usage)
}

async fn read_response(
    settings: &Settings,
    key: &str,
    payload: &Value,
    usage: &mut Usage,
) -> Option<(Value, BTreeSet<String>)> {
    let response = settings
        .http
        .post(&settings.services.ai_url)
        .timeout(std::time::Duration::from_secs(70))
        .bearer_auth(key)
        .json(payload)
        .send()
        .await
        .map_err(|error| {
            tracing::warn!(
                kind = crate::settings::transport_kind(&error),
                "AI request failed"
            );
        })
        .ok()?;
    if !response.status().is_success() {
        tracing::warn!(
            http_status = response.status().as_u16(),
            "AI request failed"
        );
        return None;
    }
    let body = response
        .json::<Value>()
        .await
        .map_err(|error| {
            tracing::warn!(
                kind = crate::settings::transport_kind(&error),
                "AI response could not be read"
            );
        })
        .ok()?;
    let tokens = |name: &str| {
        body.pointer(&format!("/usage/{name}"))
            .and_then(Value::as_i64)
            .filter(|tokens| *tokens >= 0)
    };
    usage.input_tokens = tokens("input_tokens");
    usage.output_tokens = tokens("output_tokens");
    let cached_tokens = tokens("input_tokens_details/cached_tokens");
    tracing::info!(model = %payload["model"], input_tokens = ?usage.input_tokens, ?cached_tokens, output_tokens = ?usage.output_tokens, "AI request finished");
    let text = body
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("message"))
        .flat_map(|item| {
            item.get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("output_text"))
        .filter_map(|item| item.get("text").and_then(Value::as_str))
        .collect::<String>();
    let mut sources = BTreeSet::new();
    for item in body
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let found = item
            .pointer("/action/sources")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .chain(
                item.get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|content| content.get("annotations").and_then(Value::as_array))
                    .flatten(),
            )
            .filter_map(|source| source.get("url").and_then(Value::as_str));
        sources.extend(found.map(str::to_owned));
    }
    serde_json::from_str(&text)
        .map_err(|_| tracing::warn!("AI answer is not the requested JSON"))
        .ok()
        .map(|answer| (answer, sources))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const LENGTH: usize = 280;

    fn check(text: &str) -> Result<(), &'static str> {
        valid(
            text,
            &[&Known::of(&[
                &json!({"metrics":[{"label":"Conversions","value":"412","comparison":"368","change":"12.0%"}]}),
                &json!({"title":"Campaign breakdown","rows":[["Search - Brand","148","$2,104"]]}),
            ])],
            LENGTH,
        )
    }

    #[test]
    fn ordinary_text_with_supplied_numbers_and_names_is_valid() {
        assert_eq!(
            check("Conversions grew from 368 to 412, a rise of 12.0%. Search - Brand delivered 148 of them; Protect its budget first."),
            Ok(())
        );
    }

    #[test]
    fn invented_numbers_units_names_and_causes_are_rejected() {
        assert_eq!(
            check("Conversions reached 999."),
            Err("number that is not in the data")
        );
        assert_eq!(
            check("Conversions reached 412%."),
            Err("percentage that is not in the data")
        );
        assert_eq!(
            check("Conversions grew by one million."),
            Err("number written in words")
        );
        assert_eq!(
            check("Conversions grew in Atlantis."),
            Err("name that is not in the data")
        );
        assert_eq!(
            check("The campaign Search - Invented delivered 148 conversions."),
            Err("name that is not in the data")
        );
        for cause in ["because of", "due to", "thanks to"] {
            assert_eq!(
                check(&format!("Conversions grew {cause} the new budget.")),
                Err("claimed cause")
            );
        }
        assert_eq!(check(""), Err("empty text"));
    }

    fn claimed(text: &str, claim: Claim) -> Result<(), &'static str> {
        stated_by(text, claim, &["7 October", "budget"], &[])
    }

    fn stated_by(
        text: &str,
        claim: Claim,
        anchors: &[&str],
        names: &[&str],
    ) -> Result<(), &'static str> {
        let anchors = anchors
            .iter()
            .map(|anchor| (*anchor).to_owned())
            .collect::<Vec<_>>();
        valid_monitoring(
            text,
            &[&Known::of(&[
                &json!({"metrics":[{"label":"Conversions","value":"412","comparison":"368"}]}),
                &json!({"campaigns":["Probably Shoes","Search - Real"],"days":["7 October","17 October"]}),
            ])],
            LENGTH,
            &Stated {
                claim,
                anchors: &anchors,
                names,
            },
        )
    }

    #[test]
    fn a_confirmed_cause_may_use_causal_words_in_the_sentence_that_names_it() {
        for text in [
            "Conversions grew to 412 because the budget rose.",
            "Conversions grew to 412 due to the new budget.",
            "Conversions grew to 412, driven by the budget.",
            "Conversions grew to 412 because of the change on 7 October.",
        ] {
            assert_eq!(claimed(text, Claim::Confirmed), Ok(()), "{text}");
        }
    }

    #[test]
    fn a_causal_word_in_a_sentence_without_the_cause_is_rejected_even_when_another_sentence_names_it(
    ) {
        assert_eq!(
            claimed(
                "Conversions grew to 412 because the weather changed. The budget rose.",
                Claim::Confirmed
            ),
            Err("causal word without the cause")
        );
        assert_eq!(
            claimed(
                "Conversions grew to 412 because the weather changed.",
                Claim::Confirmed
            ),
            Err("cause not stated")
        );
    }

    #[test]
    fn an_anchor_is_a_whole_word_or_date() {
        assert_eq!(
            stated_by(
                "Conversions grew to 412 because of the change on 17 October.",
                Claim::Confirmed,
                &["7 October"],
                &[]
            ),
            Err("cause not stated")
        );
    }

    #[test]
    fn a_likely_cause_must_say_that_it_is_likely_in_the_sentence_of_the_causal_word() {
        assert_eq!(
            claimed(
                "Conversions grew to 412 because the budget rose.",
                Claim::Likely
            ),
            Err("likely cause not marked as likely")
        );
        assert_eq!(
            claimed(
                "Conversions grew to 412 because the budget rose. Most likely it will hold.",
                Claim::Likely
            ),
            Err("likely cause not marked as likely")
        );
        for text in [
            "Conversions grew to 412, most likely because the budget rose.",
            "Conversions grew to 412, probably due to the budget.",
        ] {
            assert_eq!(claimed(text, Claim::Likely), Ok(()), "{text}");
        }
    }

    #[test]
    fn a_campaign_name_cannot_supply_the_likelihood_word_or_the_anchor() {
        assert_eq!(
            stated_by(
                "Conversions grew to 412 because the budget rose in Probably Shoes.",
                Claim::Likely,
                &["budget"],
                &["Probably Shoes"]
            ),
            Err("likely cause not marked as likely")
        );
        assert_eq!(
            stated_by(
                "Conversions grew to 412 because of Search - Real.",
                Claim::Confirmed,
                &["real"],
                &["Search - Real"]
            ),
            Err("cause not stated")
        );
    }

    #[test]
    fn a_contribution_or_no_cause_keeps_every_causal_word_rejected() {
        for text in [
            "Conversions grew to 412 because the budget rose.",
            "Conversions grew to 412, driven by the budget.",
            "Conversions grew to 412, which led to more calls.",
        ] {
            assert_eq!(
                claimed(text, Claim::Nothing),
                Err("claimed cause"),
                "{text}"
            );
        }
    }

    #[test]
    fn a_cause_never_excuses_an_invented_number_or_name() {
        for claim in [Claim::Confirmed, Claim::Likely] {
            assert_eq!(
                claimed(
                    "Conversions grew to 999, most likely because the budget rose.",
                    claim
                ),
                Err("number that is not in the data")
            );
            assert_eq!(
                claimed(
                    "Conversions grew to 412, most likely because Atlantis paid the budget.",
                    claim
                ),
                Err("name that is not in the data")
            );
        }
    }

    #[test]
    fn a_reason_may_be_given_only_as_a_possibility() {
        let known =
            Known::of(&[&json!({"campaigns":["Search - Real"],"changes":{"Clicks":"up 73%"}})]);
        let hedged = |text: &str, explanation: bool| {
            valid_hedged(text, &[&known], LENGTH, &["Search - Real"], explanation)
        };
        assert_eq!(
            hedged(
                "Clicks rose 73%, probably because broad match widened the searches.",
                true
            ),
            Ok(())
        );
        assert_eq!(
            hedged(
                "Clicks rose 73% because broad match widened the searches.",
                true
            ),
            Err("cause stated as a fact")
        );
        assert_eq!(
            hedged("Broad match widened the searches.", true),
            Err("explanation not marked as possible")
        );
        assert_eq!(hedged("Clicks rose 73% in Search - Real.", false), Ok(()));
    }

    #[test]
    fn money_and_percent_units_are_read_with_or_without_a_space() {
        assert_eq!(
            moneys("Spend was €412 and EUR5.00, with RON 2,493 and 30 clicks."),
            ["2493", "412", "5"].map(String::from).into()
        );
        assert_eq!(
            percents("It rose 12.0 % and fell 3%."),
            ["12", "3"].map(String::from).into()
        );
    }
}
