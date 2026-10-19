//! The claims of an Assessment. An observation states what the evidence shows; an explanation
//! offers what might lie behind it and says that it is only possible. Each point cites the
//! evidence it rests on, and its numbers and directions must agree with that evidence.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::ai::{numbers, valid, valid_hedged, Known};

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Point {
    pub observation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
}

impl From<&str> for Point {
    fn from(observation: &str) -> Self {
        Self {
            observation: observation.into(),
            explanation: None,
        }
    }
}

impl<'de> Deserialize<'de> for Point {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Stored {
            Text(String),
            Point {
                observation: String,
                #[serde(default)]
                explanation: Option<String>,
            },
        }
        Ok(match Stored::deserialize(deserializer)? {
            Stored::Text(observation) => Self {
                observation,
                explanation: None,
            },
            Stored::Point {
                observation,
                explanation,
            } => Self {
                observation,
                explanation,
            },
        })
    }
}

/// The pieces of evidence that a point can cite, by id: the result, the Ad Account, each
/// Campaign, location and Finding, the earlier Recommendations, and the business.
pub fn evidence(input: &Value) -> BTreeMap<String, Value> {
    let mut items = BTreeMap::new();
    for id in ["result", "account", "business", "conversion_lag"] {
        if !input[id].is_null() {
            items.insert(id.to_owned(), input[id].clone());
        }
    }
    for (list, field, prefix) in [
        ("campaigns", "name", "campaign"),
        ("locations", "name", "location"),
        ("findings", "key", "finding"),
        ("history", "key", "history"),
    ] {
        for item in input[list].as_array().into_iter().flatten() {
            if let Some(name) = item[field].as_str() {
                items.insert(format!("{prefix}:{name}"), item.clone());
            }
        }
    }
    items
}

const UP: [&str; 7] = ["up", "rose", "rise", "increased", "higher", "grew", "more"];
const DOWN: [&str; 7] = [
    "down",
    "fell",
    "fall",
    "decreased",
    "lower",
    "dropped",
    "less",
];

fn direction(word: &str) -> Option<bool> {
    if UP.contains(&word) {
        Some(true)
    } else if DOWN.contains(&word) {
        Some(false)
    } else {
        None
    }
}

/// The directions that the evidence gives each percent: "up 73%" makes 73 an upward change.
fn changes(evidence: &Value) -> BTreeMap<String, Vec<bool>> {
    let text = evidence.to_string().to_lowercase();
    let words = text
        .split(|character: char| !(character.is_alphanumeric() || character == '.'))
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    let mut found: BTreeMap<String, Vec<bool>> = BTreeMap::new();
    for pair in words.windows(2) {
        if let Some(up) = direction(pair[0]) {
            if let Some(number) = numbers(pair[1]).into_iter().next() {
                found.entry(number).or_default().push(up);
            }
        }
    }
    found
}

/// A percent that the text moves in the opposite direction from all of its evidence.
fn contradicts(text: &str, evidence: &Value) -> bool {
    let known = changes(evidence);
    let lower = text.to_lowercase();
    let words = lower
        .split(|character: char| {
            !(character.is_alphanumeric() || character == '.' || character == '%')
        })
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    words.iter().enumerate().any(|(index, word)| {
        if !word.ends_with('%') {
            return false;
        }
        let Some(number) = numbers(word).into_iter().next() else {
            return false;
        };
        let Some(directions) = known.get(&number) else {
            return false;
        };
        let stated = words[index.saturating_sub(5)..index]
            .iter()
            .rev()
            .find_map(|word| direction(word));
        stated.is_some_and(|up| directions.iter().all(|known| *known != up))
    })
}

/// Checks one text against the evidence it cites: only names and numbers of that evidence,
/// each number as displayed, and no direction that contradicts it. An explanation may give a
/// reason only as a possibility; an observation gives none.
pub fn grounded(
    text: &str,
    cited: &Value,
    length: usize,
    names: &[&str],
    explanation: bool,
) -> Result<(), &'static str> {
    let known = Known::of(&[cited]);
    if explanation {
        valid_hedged(text, &[&known], length, names, true)?;
    } else {
        valid(text, &[&known], length)?;
    }
    if !super::update::numeric_tokens(text)
        .is_subset(&super::update::numeric_tokens(&cited.to_string()))
    {
        return Err("number that its evidence does not hold");
    }
    if contradicts(text, cited) {
        return Err("direction that contradicts its evidence");
    }
    Ok(())
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct Answer {
    #[serde(default)]
    pub observation: String,
    #[serde(default)]
    pub explanation: Option<String>,
    #[serde(default)]
    pub evidence: Vec<String>,
}

/// The point that an answer makes, when every text of it rests on the evidence it cites.
pub(crate) fn point(
    answer: Answer,
    items: &BTreeMap<String, Value>,
    length: usize,
    names: &[&str],
) -> Result<Point, &'static str> {
    if answer.evidence.is_empty() {
        return Err("no evidence cited");
    }
    let cited = answer
        .evidence
        .iter()
        .map(|id| items.get(id).cloned().ok_or("unknown evidence"))
        .collect::<Result<Vec<_>, _>>()?;
    let cited = json!([cited, items.get("result")]);
    grounded(&answer.observation, &cited, length, names, false)?;
    if numbers(&answer.observation).is_empty() {
        return Err("observation without a number");
    }
    let explanation = answer
        .explanation
        .filter(|text| !text.trim().is_empty())
        .map(|text| grounded(&text, &cited, length, names, true).map(|()| text))
        .transpose()?;
    Ok(Point {
        observation: answer.observation,
        explanation,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> BTreeMap<String, Value> {
        evidence(&json!({
            "result": {"verdict": "worse"},
            "account": {"changes": {"Clicks": "up 73%", "Cost per conversion": "up 46%"}},
            "campaigns": [{"name": "Search - Drain Cleaning", "changes": {"Conversions": "up 4%"}, "last_30_days": {"Spend": "$4,556"}}],
            "findings": [{"key": "search-terms-no-conversions:31", "detail": "Search - Drain Cleaning had $2,110 spend and 0 conversions on 5 search terms."}],
        }))
    }

    fn answer(observation: &str, explanation: Option<&str>, evidence: &[&str]) -> Answer {
        Answer {
            observation: observation.into(),
            explanation: explanation.map(Into::into),
            evidence: evidence.iter().map(|id| (*id).to_owned()).collect(),
        }
    }

    fn said(
        observation: &str,
        explanation: Option<&str>,
        evidence: &[&str],
    ) -> Result<Point, &'static str> {
        point(
            answer(observation, explanation, evidence),
            &items(),
            280,
            &["Search - Drain Cleaning"],
        )
    }

    #[test]
    fn an_observation_rests_on_its_cited_evidence_and_an_explanation_says_that_it_is_possible() {
        assert_eq!(
            said(
                "Clicks rose 73% while cost per conversion rose 46%.",
                Some("The extra clicks may come from searches with no intent to buy, because broad match likely widened them."),
                &["account"]
            ),
            Ok(Point {
                observation: "Clicks rose 73% while cost per conversion rose 46%.".into(),
                explanation: Some("The extra clicks may come from searches with no intent to buy, because broad match likely widened them.".into()),
            })
        );
        assert_eq!(
            said(
                "Clicks rose 73% because broad match widened the searches.",
                None,
                &["account"]
            ),
            Err("claimed cause")
        );
        assert_eq!(
            said(
                "Clicks rose 73%.",
                Some("Broad match widened the searches."),
                &["account"]
            ),
            Err("explanation not marked as possible")
        );
    }

    #[test]
    fn a_number_from_other_evidence_or_a_wrong_direction_is_rejected() {
        assert_eq!(
            said(
                "Search terms with no conversions took $2,110.",
                None,
                &["account"]
            ),
            Err("number that is not in the data")
        );
        assert_eq!(
            said(
                "Search terms with no conversions took $2,110.",
                None,
                &["finding:search-terms-no-conversions:31"]
            )
            .map(|point| point.observation),
            Ok("Search terms with no conversions took $2,110.".into())
        );
        assert_eq!(
            said("Clicks fell 73% over the period.", None, &["account"]),
            Err("direction that contradicts its evidence")
        );
        assert_eq!(
            said("Clicks rose sharply.", None, &["account"]),
            Err("observation without a number")
        );
        assert_eq!(
            said("Clicks rose 73%.", None, &["campaign:Search - Other"]),
            Err("unknown evidence")
        );
        assert_eq!(
            said("Clicks rose 73%.", None, &[]),
            Err("no evidence cited")
        );
    }

    #[test]
    fn a_point_stored_as_plain_text_is_read_as_an_observation() {
        let point: Point = serde_json::from_value(json!("Clicks rose 73%.")).unwrap();
        assert_eq!(point.observation, "Clicks rose 73%.");
        assert_eq!(point.explanation, None);
    }
}
