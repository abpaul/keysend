//! The answer of an investigation. The model drafts it as claims that cite evidence IDs; code
//! keeps a claim only when its numbers, names and directions occur in the evidence it cites,
//! sets each strength from that evidence, and adds every caveat of the cited evidence as a
//! limitation. An observation states no reason; a hypothesis and a Recommendation give one
//! only as a possibility.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::evidence::{Caveat, Evidence, EvidenceId, Strength, Subject};
use crate::monitoring::claims::grounded;

pub const SUMMARY_LENGTH: usize = 600;
pub const CLAIM_LENGTH: usize = 320;
pub const CLAIMS: usize = 6;
pub const RECOMMENDATIONS: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A change to make in the Ad Account.
    Act,
    /// Something to look at before any change.
    Investigate,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct DraftClaim {
    pub text: String,
    pub evidence: Vec<EvidenceId>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DraftRecommendation {
    pub kind: Kind,
    pub action: String,
    pub target: String,
    pub evidence: Vec<EvidenceId>,
    pub rationale: String,
    pub risk: String,
    pub measure: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Draft {
    pub summary: String,
    #[serde(default)]
    pub observations: Vec<DraftClaim>,
    #[serde(default)]
    pub hypotheses: Vec<DraftClaim>,
    #[serde(default)]
    pub recommendations: Vec<DraftRecommendation>,
    #[serde(default)]
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Claim {
    pub text: String,
    pub evidence: Vec<EvidenceId>,
    pub strength: Strength,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recommendation {
    pub kind: Kind,
    pub action: String,
    pub target: String,
    pub evidence: Vec<EvidenceId>,
    pub rationale: String,
    pub risk: String,
    pub measure: String,
    pub strength: Strength,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Limitation {
    /// A caveat that code attached to cited evidence.
    Evidence {
        evidence: EvidenceId,
        caveat: Caveat,
    },
    /// A limitation in the words of the model, checked like a hypothesis.
    Stated { text: String },
}

/// What the User reads. `summary` is absent when no summary rested on the kept claims; the
/// page then leads with the observations.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub summary: Option<String>,
    pub observations: Vec<Claim>,
    pub hypotheses: Vec<Claim>,
    pub recommendations: Vec<Recommendation>,
    pub limitations: Vec<Limitation>,
    /// The cited evidence, in the order of first citation, so that the page can show it.
    pub evidence: Vec<Evidence>,
    /// How many drafted texts the check refused.
    pub refused: usize,
}

/// The answer part of the strict schema. Evidence IDs are an enum of the IDs that exist, so
/// the model cannot cite evidence that no operation returned.
pub fn schema(ids: &[EvidenceId]) -> Value {
    let ids = ids.iter().map(|id| id.0.clone()).collect::<Vec<_>>();
    let cited = if ids.is_empty() {
        json!({"type":"array","maxItems":0,"items":{"type":"string"}})
    } else {
        json!({"type":"array","minItems":1,"maxItems":4,"items":{"type":"string","enum":ids}})
    };
    let claim = json!({"type":"object","additionalProperties":false,"required":["text","evidence"],"properties":{
        "text":{"type":"string","maxLength":CLAIM_LENGTH},
        "evidence":cited,
    }});
    let text = json!({"type":"string","maxLength":CLAIM_LENGTH});
    json!({"type":"object","additionalProperties":false,
    "required":["summary","observations","hypotheses","recommendations","limitations"],
    "properties":{
        "summary":{"type":"string","maxLength":SUMMARY_LENGTH},
        "observations":{"type":"array","maxItems":CLAIMS,"items":claim},
        "hypotheses":{"type":"array","maxItems":CLAIMS,"items":claim},
        "recommendations":{"type":"array","maxItems":RECOMMENDATIONS,"items":{
            "type":"object","additionalProperties":false,
            "required":["kind","action","target","evidence","rationale","risk","measure"],
            "properties":{
                "kind":{"type":"string","enum":["act","investigate"]},
                "action":text,"target":text,"evidence":cited,
                "rationale":text,"risk":text,"measure":text,
            }}},
        "limitations":{"type":"array","maxItems":CLAIMS,"items":text},
    }})
}

struct Cited<'a> {
    items: Vec<&'a Evidence>,
    value: Value,
    strength: Strength,
}

/// What a claim may repeat from a piece of evidence: its subject, its facts and the numbers
/// of its caveats, such as the days not yet settled.
fn shown(item: &Evidence) -> Value {
    json!({"subject": item.subject, "facts": item.facts, "caveats": item.caveats})
}

/// Dashes and double quotes become plain punctuation, which the shared check accepts.
/// Evidence IDs written into the text, such as `[E2.1, E8.2]`, are removed: the citations
/// carry them, and the number check would read `E8.2` as the number 8.2.
fn tidy(text: String) -> String {
    let text = text
        .replace(" \u{2014} ", ", ")
        .replace(['\u{2014}', '\u{2013}'], "-")
        .replace('"', "'");
    let characters = text.chars().collect::<Vec<_>>();
    let mut kept = String::with_capacity(text.len());
    let mut index = 0;
    while index < characters.len() {
        let id = characters[index] == 'E'
            && (index == 0 || !characters[index - 1].is_alphanumeric())
            && {
                let mut end = index + 1;
                let start = end;
                while end < characters.len() && characters[end].is_ascii_digit() {
                    end += 1;
                }
                let step = end > start;
                let dot = end < characters.len() && characters[end] == '.';
                let mut after = end + 1;
                while after < characters.len() && characters[after].is_ascii_digit() {
                    after += 1;
                }
                if step && dot && after > end + 1 {
                    index = after;
                    true
                } else {
                    false
                }
            };
        if !id {
            kept.push(characters[index]);
            index += 1;
        }
    }
    kept = without_empty_brackets(&kept);
    for (from, to) in [(", ,", ","), ("  ", " "), (" .", "."), (" ,", ",")] {
        while kept.contains(from) {
            kept = kept.replace(from, to);
        }
    }
    kept.trim().to_owned()
}

/// Removes a bracket group that holds only separators, such as `[, ]` or `(; )`.
fn without_empty_brackets(text: &str) -> String {
    let mut kept = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find(['[', '(']) {
        let close = if rest[open..].starts_with('[') {
            ']'
        } else {
            ')'
        };
        let inner_end = rest[open + 1..].find(close).map(|end| open + 1 + end);
        match inner_end {
            Some(end)
                if rest[open + 1..end].chars().all(|character| {
                    character.is_whitespace() || matches!(character, ',' | ';')
                }) =>
            {
                kept.push_str(&rest[..open]);
                rest = &rest[end + 1..];
            }
            _ => {
                kept.push_str(&rest[..=open]);
                rest = &rest[open + 1..];
            }
        }
    }
    kept.push_str(rest);
    kept
}

/// Whether a JSON value holds `target` as one whole string value, such as the name of a
/// Campaign in a list of Findings.
fn holds_string(value: &Value, target: &str) -> bool {
    match value {
        Value::String(text) => normalized(text) == target,
        Value::Array(items) => items.iter().any(|item| holds_string(item, target)),
        Value::Object(fields) => fields.values().any(|item| holds_string(item, target)),
        _ => false,
    }
}

fn cite<'a>(
    ids: &[EvidenceId],
    known: &'a BTreeMap<&EvidenceId, &'a Evidence>,
    context: &Value,
) -> Result<Cited<'a>, &'static str> {
    if ids.is_empty() {
        return Err("no evidence cited");
    }
    let items = ids
        .iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|id| known.get(id).copied().ok_or("unknown evidence"))
        .collect::<Result<Vec<_>, _>>()?;
    let strength = items
        .iter()
        .map(|item| item.strength)
        .min()
        .unwrap_or(Strength::Weak);
    let value = json!([
        items.iter().map(|item| shown(item)).collect::<Vec<_>>(),
        context
    ]);
    Ok(Cited {
        items,
        value,
        strength,
    })
}

/// A text that states what the evidence shows, or gives a reason only as a possibility.
fn stated(text: &str, cited: &Value, length: usize, names: &[&str]) -> Result<(), &'static str> {
    grounded(text, cited, length, names, false)
        .or_else(|_| grounded(text, cited, length, names, true))
}

fn normalized(text: &str) -> String {
    text.to_lowercase()
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Checks a draft against the evidence that the operations of the investigation returned.
/// `names` are the Campaign and Ad Account names, removed before the causal check so that a
/// name cannot supply a causal word.
/// `context` is what the request itself establishes and a text may repeat: the period, the
/// day of the question, the Ad Account names and the terms of Google Ads.
pub fn check(draft: Draft, evidence: &[Evidence], names: &[&str], context: &Value) -> Report {
    let known = evidence
        .iter()
        .map(|item| (&item.id, item))
        .collect::<BTreeMap<_, _>>();
    let mut refused = 0;
    let mut used = Vec::<EvidenceId>::new();
    let mut note = |reason: &'static str, part: &'static str| {
        tracing::info!(reason, part, "Investigation text refused");
        refused += 1;
    };
    let mut cite_used = |ids: &[EvidenceId]| {
        for id in ids {
            if !used.contains(id) {
                used.push(id.clone());
            }
        }
    };

    let mut claims = |drafts: Vec<DraftClaim>, explanation: bool, what: &'static str| {
        let mut kept = Vec::<Claim>::new();
        for mut draft in drafts.into_iter().take(CLAIMS) {
            draft.text = tidy(draft.text);
            let checked = cite(&draft.evidence, &known, context).and_then(|cited| {
                grounded(&draft.text, &cited.value, CLAIM_LENGTH, names, explanation)
                    .map(|()| cited.strength)
            });
            match checked {
                Ok(_)
                    if kept
                        .iter()
                        .any(|claim| normalized(&claim.text) == normalized(&draft.text)) =>
                {
                    note("repeated", what);
                }
                Ok(strength) => kept.push(Claim {
                    text: draft.text,
                    evidence: draft.evidence,
                    strength,
                }),
                Err(reason) => note(reason, what),
            }
        }
        kept
    };
    let observations = claims(draft.observations, false, "observation");
    let hypotheses = claims(draft.hypotheses, true, "hypothesis");

    let mut recommendations = Vec::<Recommendation>::new();
    for mut draft in draft.recommendations.into_iter().take(RECOMMENDATIONS) {
        for text in [
            &mut draft.action,
            &mut draft.target,
            &mut draft.rationale,
            &mut draft.risk,
            &mut draft.measure,
        ] {
            *text = tidy(std::mem::take(text));
        }
        let checked = cite(&draft.evidence, &known, context).and_then(|cited| {
            for text in [
                &draft.action,
                &draft.target,
                &draft.rationale,
                &draft.risk,
                &draft.measure,
            ] {
                stated(text, &cited.value, CLAIM_LENGTH, names)?;
            }
            let target = normalized(&draft.target);
            let named = cited.items.iter().any(|item| match &item.subject {
                Subject::AdAccount => {
                    matches!(target.as_str(), "ad account" | "the ad account")
                        || context["ad_accounts"]
                            .get(item.account.index())
                            .and_then(Value::as_str)
                            .is_some_and(|name| normalized(name) == target)
                }
                subject => subject
                    .name()
                    .is_some_and(|name| normalized(name) == target),
            });
            let mentioned = || {
                names.iter().any(|name| normalized(name) == target)
                    && cited
                        .items
                        .iter()
                        .any(|item| holds_string(&item.facts, &target))
            };
            if !named && !mentioned() {
                return Err("target that its evidence does not name");
            }
            if draft.kind == Kind::Act && cited.strength == Strength::Weak {
                return Err("action on weak evidence");
            }
            Ok(cited.strength)
        });
        match checked {
            Ok(_)
                if recommendations
                    .iter()
                    .any(|kept| normalized(&kept.action) == normalized(&draft.action)) =>
            {
                note("repeated", "recommendation");
            }
            Ok(strength) => recommendations.push(Recommendation {
                kind: draft.kind,
                action: draft.action,
                target: draft.target,
                evidence: draft.evidence,
                rationale: draft.rationale,
                risk: draft.risk,
                measure: draft.measure,
                strength,
            }),
            Err(reason) => note(reason, "recommendation"),
        }
    }

    for ids in observations
        .iter()
        .chain(&hypotheses)
        .map(|claim| &claim.evidence)
        .chain(recommendations.iter().map(|kept| &kept.evidence))
    {
        cite_used(ids);
    }
    let cited = used
        .iter()
        .filter_map(|id| known.get(id).copied())
        .collect::<Vec<_>>();
    let all = json!([
        cited.iter().map(|item| shown(item)).collect::<Vec<_>>(),
        context
    ]);

    let summary = (!cited.is_empty())
        .then(|| tidy(draft.summary))
        .filter(|summary| {
            stated(summary, &all, SUMMARY_LENGTH, names)
                .map_err(|reason| note(reason, "summary"))
                .is_ok()
        });

    let mut limitations = Vec::<Limitation>::new();
    for item in &cited {
        for caveat in &item.caveats {
            if !limitations.iter().any(
                |kept| matches!(kept, Limitation::Evidence { caveat: known, .. } if known == caveat),
            ) {
                limitations.push(Limitation::Evidence {
                    evidence: item.id.clone(),
                    caveat: caveat.clone(),
                });
            }
        }
    }
    for text in draft.limitations.into_iter().take(CLAIMS).map(tidy) {
        match stated(&text, &all, CLAIM_LENGTH, names) {
            Ok(()) => limitations.push(Limitation::Stated { text }),
            Err(reason) => note(reason, "limitation"),
        }
    }

    Report {
        summary,
        observations,
        hypotheses,
        recommendations,
        limitations,
        evidence: cited.into_iter().cloned().collect(),
        refused,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{
        evidence::{Days, Source},
        Handle,
    };

    fn item(id: &str, subject: Subject, facts: Value, strength: Strength) -> Evidence {
        Evidence {
            id: EvidenceId(id.into()),
            account: Handle::new(0),
            source: Source::AccountHistory,
            subject,
            days: Some(Days {
                start: 100,
                end: 129,
            }),
            baseline: Some(Days { start: 70, end: 99 }),
            facts,
            strength,
            caveats: Vec::new(),
        }
    }

    fn brand() -> Subject {
        Subject::Campaign {
            id: 31,
            name: "Search - Brand".into(),
        }
    }

    fn evidence() -> Vec<Evidence> {
        let mut account = item(
            "E1.1",
            Subject::AdAccount,
            json!({"Cost per conversion": "$48.10", "Change in cost per conversion": "up 35%", "Clicks": "1,204", "Change in clicks": "up 2%"}),
            Strength::Strong,
        );
        account
            .caveats
            .push(Caveat::ConversionsNotSettled { days: 2 });
        let campaign = item(
            "E2.1",
            brand(),
            json!({"Spend": "$2,110", "Conversions": "4", "Change in conversions": "down 60%"}),
            Strength::Weak,
        );
        let mut unrelated = item(
            "E3.1",
            Subject::Segment {
                name: "Mobile".into(),
            },
            json!({"Spend": "$990"}),
            Strength::Strong,
        );
        unrelated.caveats.push(Caveat::Snapshot { day: 120 });
        vec![account, campaign, unrelated]
    }

    fn claim(text: &str, ids: &[&str]) -> DraftClaim {
        DraftClaim {
            text: text.into(),
            evidence: ids.iter().map(|id| EvidenceId((*id).into())).collect(),
        }
    }

    fn recommendation(kind: Kind, action: &str, target: &str, ids: &[&str]) -> DraftRecommendation {
        DraftRecommendation {
            kind,
            action: action.into(),
            target: target.into(),
            evidence: ids.iter().map(|id| EvidenceId((*id).into())).collect(),
            rationale: "Its conversions fell by 60%.".into(),
            risk: "Few conversions may make the change noise.".into(),
            measure: "Conversions of Search - Brand after the review.".into(),
        }
    }

    fn checked(draft: Draft) -> Report {
        check(
            draft,
            &evidence(),
            &["Search - Brand"],
            &json!({"period": "last 30 days"}),
        )
    }

    #[test]
    fn a_user_never_reads_an_injected_ad_account_target() {
        for (target, expected) in [
            ("Contact support at help@example.com", 0),
            ("Another Ad Account", 0),
            ("Ad Account 999999", 0),
            ("Harbour Dental", 1),
            ("harbour dental", 1),
            ("Ad Account", 1),
            ("the Ad Account", 1),
        ] {
            let draft = Draft {
                recommendations: vec![DraftRecommendation {
                    kind: Kind::Investigate,
                    action: "Review cost per conversion.".into(),
                    target: target.into(),
                    evidence: vec![EvidenceId("E1.1".into())],
                    rationale: "Cost per conversion rose 35%.".into(),
                    risk: "Few conversions may make the change noise.".into(),
                    measure: "Cost per conversion after the review.".into(),
                }],
                ..Draft::default()
            };
            let report = check(
                draft,
                &evidence(),
                &["Harbour Dental", "Another Ad Account"],
                &json!({"ad_accounts":["Harbour Dental", "Another Ad Account"], "terms":"Ad Account"}),
            );
            assert_eq!(report.recommendations.len(), expected, "target: {target}");
        }
    }

    #[test]
    fn a_claim_keeps_only_numbers_of_the_evidence_it_cites() {
        let report = checked(Draft {
            summary: "Cost per conversion rose 35% while clicks held.".into(),
            observations: vec![
                claim("Cost per conversion rose 35% to $48.10.", &["E1.1"]),
                claim("Search - Brand spent $990.", &["E2.1"]),
            ],
            ..Draft::default()
        });
        assert_eq!(
            report
                .observations
                .iter()
                .map(|claim| claim.text.as_str())
                .collect::<Vec<_>>(),
            ["Cost per conversion rose 35% to $48.10."]
        );
        assert_eq!(report.refused, 1);
        assert_eq!(
            report.summary.as_deref(),
            Some("Cost per conversion rose 35% while clicks held.")
        );
    }

    #[test]
    fn an_observation_states_no_reason_and_a_hypothesis_states_one_as_possible() {
        let report = checked(Draft {
            summary: String::new(),
            observations: vec![claim(
                "Cost per conversion rose 35% because clicks rose 2%.",
                &["E1.1"],
            )],
            hypotheses: vec![
                claim(
                    "Cost per conversion rose 35% because clicks rose 2%.",
                    &["E1.1"],
                ),
                claim(
                    "Cost per conversion may have risen 35% because fewer clicks converted.",
                    &["E1.1"],
                ),
            ],
            ..Draft::default()
        });
        assert!(report.observations.is_empty());
        assert_eq!(report.hypotheses.len(), 1);
        assert_eq!(report.refused, 3);
        assert_eq!(report.summary, None);
    }

    #[test]
    fn a_change_on_weak_evidence_is_refused_and_an_investigation_is_kept() {
        let report = checked(Draft {
            summary: String::new(),
            recommendations: vec![
                recommendation(
                    Kind::Act,
                    "Lower the budget of Search - Brand.",
                    "Search - Brand",
                    &["E2.1"],
                ),
                recommendation(
                    Kind::Investigate,
                    "Review the search terms of Search - Brand.",
                    "Search - Brand",
                    &["E2.1"],
                ),
            ],
            ..Draft::default()
        });
        assert_eq!(report.recommendations.len(), 1);
        assert_eq!(report.recommendations[0].kind, Kind::Investigate);
        assert_eq!(report.recommendations[0].strength, Strength::Weak);
    }

    #[test]
    fn a_recommendation_names_a_subject_of_its_evidence() {
        let report = checked(Draft {
            summary: String::new(),
            recommendations: vec![recommendation(
                Kind::Investigate,
                "Review the search terms of Search - Brand.",
                "Search - Generic",
                &["E2.1"],
            )],
            ..Draft::default()
        });
        assert!(report.recommendations.is_empty());
        assert_eq!(report.refused, 1);
    }

    #[test]
    fn the_caveats_of_cited_evidence_become_limitations() {
        let report = checked(Draft {
            summary: String::new(),
            observations: vec![claim("Cost per conversion rose 35%.", &["E1.1"])],
            ..Draft::default()
        });
        assert_eq!(
            report.limitations,
            [Limitation::Evidence {
                evidence: EvidenceId("E1.1".into()),
                caveat: Caveat::ConversionsNotSettled { days: 2 },
            }]
        );
        assert_eq!(report.evidence.len(), 1);
    }

    #[test]
    fn a_claim_cites_evidence_that_an_operation_returned() {
        let report = checked(Draft {
            summary: String::new(),
            observations: vec![
                claim("Cost per conversion rose 35%.", &["E9.9"]),
                claim("Cost per conversion rose 35%.", &[]),
            ],
            ..Draft::default()
        });
        assert!(report.observations.is_empty());
        assert_eq!(report.refused, 2);
    }

    #[test]
    fn a_claim_takes_the_strength_of_its_weakest_evidence() {
        let report = checked(Draft {
            summary: String::new(),
            observations: vec![claim(
                "Cost per conversion rose 35% and Search - Brand had 4 conversions.",
                &["E1.1", "E2.1"],
            )],
            ..Draft::default()
        });
        assert_eq!(report.observations[0].strength, Strength::Weak);
    }

    #[test]
    fn a_summary_may_repeat_the_period_a_caveat_and_plain_dashes() {
        let report = check(
            Draft {
                summary: "Over the last 30 days CPA rose 35% \u{2014} and the last 2 days are not settled.".into(),
                observations: vec![claim("Cost per conversion rose 35%.", &["E1.1"])],
                limitations: vec!["The last 2 days still miss conversions.".into()],
                ..Draft::default()
            },
            &evidence(),
            &["Search - Brand"],
            &json!({"period": "last 30 days", "terms": "CPA"}),
        );
        assert_eq!(
            report.summary.as_deref(),
            Some("Over the last 30 days CPA rose 35%, and the last 2 days are not settled.")
        );
        assert_eq!(report.refused, 0);
        assert!(report.limitations.contains(&Limitation::Stated {
            text: "The last 2 days still miss conversions.".into()
        }));
    }

    #[test]
    fn evidence_ids_written_into_a_text_are_removed_before_its_check() {
        assert_eq!(
            tidy("Cost per conversion rose 35% [E1.1, E2.1]. Search - Brand fell (E2.1).".into()),
            "Cost per conversion rose 35%. Search - Brand fell."
        );
        assert_eq!(tidy("EUR 100 in E1.1 and E12.3".into()), "EUR 100 in and");
        assert_eq!(
            tidy("Conversions held. [E1.1, E2.1; E3.1] (E1.2) [kept]".into()),
            "Conversions held. [kept]"
        );
        let report = checked(Draft {
            summary: "Cost per conversion rose 35% [E1.1, E8.2].".into(),
            observations: vec![claim("Cost per conversion rose 35% (E1.1).", &["E1.1"])],
            ..Draft::default()
        });
        assert_eq!(
            report.summary.as_deref(),
            Some("Cost per conversion rose 35%.")
        );
        assert_eq!(report.refused, 0);
    }

    #[test]
    fn a_recommendation_may_target_a_campaign_that_its_ad_account_evidence_names() {
        let mut ranking = item(
            "E1.1",
            Subject::AdAccount,
            json!({"Active Findings": [{"Subject": "Search - Brand", "Spend": "$2,110", "Change in conversions": "down 60%"}]}),
            Strength::Strong,
        );
        ranking.caveats.clear();
        let target = |target: &str| {
            check(
                Draft {
                    summary: String::new(),
                    recommendations: vec![recommendation(
                        Kind::Investigate,
                        "Review the search terms of Search - Brand.",
                        target,
                        &["E1.1"],
                    )],
                    ..Draft::default()
                },
                &[ranking.clone()],
                &["Search - Brand", "Search - Generic"],
                &json!({"ad_accounts": ["Northwind Dental"]}),
            )
            .recommendations
            .len()
        };
        assert_eq!(target("Search - Brand"), 1);
        assert_eq!(target("Search - Generic"), 0);
        assert_eq!(target("Search"), 0);
        assert_eq!(target("Contact support at help"), 0);
    }
}
