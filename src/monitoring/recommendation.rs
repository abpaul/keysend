use serde_json::{json, Value};

use super::{audit::AccountChange, deep_check::Finding};
use crate::ai::{numbers, valid, Known};

pub const INTENTS: [&str; 7] = [
    "reduce_spend",
    "pause",
    "exclude",
    "improve",
    "investigate",
    "increase_budget",
    "keep",
];
pub const ACTION_LENGTH: usize = 140;
pub const REASON_LENGTH: usize = 260;

pub(crate) struct Proposal {
    pub finding: String,
    pub action: String,
    pub reason: String,
    pub evidence: String,
}

fn contains_any(text: &str, parts: &[&str]) -> bool {
    parts.iter().any(|part| text.contains(part))
}

fn permitted(finding: &str) -> &'static [&'static str] {
    match finding.split(':').next().unwrap_or_default() {
        "search-terms-no-conversions" | "keywords-no-conversions" => {
            &["exclude", "pause", "reduce_spend", "investigate"]
        }
        "budget-limited" => &["increase_budget", "investigate", "keep"],
        "landing-page-low-rate" | "ads-poor-strength" => &["improve", "investigate"],
        "tracking-not-set-up" | "tracking-no-results" => &[],
        _ => &["reduce_spend", "pause", "improve", "investigate"],
    }
}

pub(crate) fn validate(
    entry: &Value,
    findings: &[Finding],
    changes: &[AccountChange],
    account: &Known,
) -> Result<Proposal, &'static str> {
    let field = |name: &str| entry.get(name).and_then(Value::as_str).unwrap_or_default();
    let (action, reason, intent) = (field("action"), field("reason"), field("intent"));
    let finding = findings
        .iter()
        .find(|finding| finding.rule.key == field("finding"))
        .ok_or("no Finding supports it")?;
    let campaign_id = finding.campaign_id;
    let finding = &finding.rule;
    if !permitted(&finding.key).contains(&intent) {
        return Err("its Finding does not support this kind of action");
    }
    let known = Known::of(&[&json!(finding)]);
    valid(action, &[account, &known], ACTION_LENGTH)?;
    valid(reason, &[account, &known], REASON_LENGTH)?;
    let lower = action.to_lowercase();
    if !finding.subjects.is_empty()
        && !finding
            .subjects
            .iter()
            .any(|subject| lower.contains(&subject.to_lowercase()))
    {
        return Err("it does not name a subject of its Finding");
    }
    for (required, wording) in [
        (
            "pause",
            &["pause", "stop ", "turn off", "disable", "switch off"][..],
        ),
        ("exclude", &["negative keyword", "exclude", "block "][..]),
    ] {
        if intent != required && contains_any(&lower, wording) {
            return Err("its wording does not match its intent");
        }
    }
    if !matches!(intent, "reduce_spend" | "pause")
        && contains_any(&lower, &["budget", "bid", "spend"])
        && contains_any(&lower, &["reduce", "lower", "decrease", "cut "])
    {
        return Err("its wording does not match its intent");
    }
    let more = intent == "increase_budget";
    let less = matches!(intent, "reduce_spend" | "pause");
    if !more
        && contains_any(&lower, &["budget", "bid", "spend"])
        && contains_any(
            &lower,
            &[
                "increase",
                "raise",
                "double",
                "triple",
                "boost",
                "higher",
                "more ",
                "scale up",
                "grow",
                "lift",
                "remove the budget limit",
            ],
        )
    {
        return Err("it asks for more spend without a Finding for that");
    }
    for change in changes {
        if change.campaign.is_empty() || !lower.contains(&change.campaign.to_lowercase()) {
            continue;
        }
        if change
            .campaign_id
            .zip(campaign_id)
            .is_some_and(|(changed, found)| changed != found)
        {
            continue;
        }
        let Some((from, to)) = change
            .text
            .strip_prefix("Daily budget changed from ")
            .and_then(|rest| rest.split_once(" to "))
        else {
            continue;
        };
        let amount = |text: &str| {
            numbers(text)
                .into_iter()
                .next()
                .and_then(|number| number.parse::<f64>().ok())
        };
        if let Some((from, to)) = amount(from).zip(amount(to)) {
            if (to < from && more) || (to > from && less) {
                return Err("it reverses a recent Account change");
            }
        }
    }
    Ok(Proposal {
        finding: finding.key.clone(),
        action: action.into(),
        reason: reason.into(),
        evidence: finding.detail.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(campaign_id: i64, key: &str, title: &str, detail: &str, subject: &str) -> Finding {
        Finding {
            campaign_id: Some(campaign_id),
            rule: crate::monitoring::findings::Finding {
                key: key.into(),
                title: title.into(),
                detail: detail.into(),
                subjects: vec![subject.into()],
                spend: 300.0,
            },
        }
    }

    fn findings() -> Vec<Finding> {
        vec![
            finding(
                701,
                "campaign-cost-high:Display - Remarketing",
                "Display - Remarketing has a high cost per conversion",
                "Display - Remarketing spent $400 for 4 conversions ($100 per conversion). The account cost per conversion is $40.00.",
                "Display - Remarketing",
            ),
            finding(
                702,
                "budget-limited:702:Porta (702)",
                "Porta is limited by its budget",
                "Porta lost 22% of its search impressions to budget.",
                "Porta",
            ),
            finding(
                703,
                "budget-limited:703:Porta (703)",
                "Porta is limited by its budget",
                "Porta lost 22% of its search impressions to budget.",
                "Porta",
            ),
        ]
    }

    fn recommend(
        finding: &str,
        intent: &str,
        action: &str,
        reason: &str,
    ) -> Result<String, &'static str> {
        let changes = vec![AccountChange {
            date: "2026-09-12".into(),
            campaign: "Porta".into(),
            campaign_id: Some(702),
            text: "Daily budget changed from $200.00 to $80.00".into(),
        }];
        let account = Known::of(&[&json!(changes), &json!("$40.00")]);
        validate(
            &json!({"finding": finding, "intent": intent, "action": action, "reason": reason}),
            &findings(),
            &changes,
            &account,
        )
        .map(|row| row.evidence)
    }

    #[test]
    fn a_recommendation_needs_a_finding_that_permits_its_intent_and_names_its_subject() {
        const HIGH: &str = "campaign-cost-high:Display - Remarketing";
        assert_eq!(
            recommend(
                HIGH,
                "reduce_spend",
                "Limit Display - Remarketing to its best audiences.",
                "It spent $400 for 4 conversions, far above the account value of $40.00."
            ),
            Ok("Display - Remarketing spent $400 for 4 conversions ($100 per conversion). The account cost per conversion is $40.00.".into())
        );
        assert_eq!(
            recommend(
                "metric-1",
                "pause",
                "Pause all campaigns.",
                "Conversions increased."
            ),
            Err("no Finding supports it")
        );
        assert_eq!(
            recommend(
                HIGH,
                "pause",
                "Pause all campaigns.",
                "It spent $400 for 4 conversions."
            ),
            Err("it does not name a subject of its Finding")
        );
        assert_eq!(
            recommend(
                HIGH,
                "improve",
                "Review Display - Remarketing.",
                "It lost 22% of its impressions."
            ),
            Err("number that is not in the data")
        );
        assert_eq!(
            recommend(
                HIGH,
                "improve",
                "Review Display - Remarketing.",
                "It spent 400% more."
            ),
            Err("percentage that is not in the data")
        );
        assert_eq!(
            recommend(
                HIGH,
                "improve",
                "Review Display - Remarketing.",
                "It got 4 conversions for $4."
            ),
            Err("amount of money that is not in the data")
        );
        assert_eq!(
            recommend(
                HIGH,
                "increase_budget",
                "Increase the budget of Display - Remarketing.",
                "It spent $400."
            ),
            Err("its Finding does not support this kind of action")
        );
        for action in [
            "Double the budget of Display - Remarketing.",
            "Boost Display - Remarketing bids.",
            "Remove the budget limit on Display - Remarketing.",
        ] {
            assert_eq!(
                recommend(HIGH, "improve", action, "It spent $400."),
                Err("it asks for more spend without a Finding for that")
            );
        }
        assert!(recommend(
            HIGH,
            "investigate",
            "Increase the tracking coverage of Display - Remarketing.",
            "It spent $400 for 4 conversions."
        )
        .is_ok());
    }

    #[test]
    fn the_wording_of_a_recommendation_matches_its_intent() {
        const HIGH: &str = "campaign-cost-high:Display - Remarketing";
        for (intent, action) in [
            ("investigate", "Pause Display - Remarketing."),
            ("improve", "Add negative keywords to Display - Remarketing."),
            ("improve", "Reduce the budget of Display - Remarketing."),
        ] {
            assert_eq!(
                recommend(HIGH, intent, action, "It spent $400."),
                Err("its wording does not match its intent")
            );
        }
        assert!(recommend(
            HIGH,
            "pause",
            "Pause Display - Remarketing.",
            "It spent $400."
        )
        .is_ok());
        assert!(recommend(
            HIGH,
            "reduce_spend",
            "Reduce the budget of Display - Remarketing.",
            "It spent $400."
        )
        .is_ok());
    }

    #[test]
    fn a_recommendation_does_not_reverse_a_recent_budget_change() {
        assert_eq!(
            recommend(
                "budget-limited:702:Porta (702)",
                "increase_budget",
                "Increase the daily budget of Porta.",
                "Porta lost 22% of its search impressions to budget."
            ),
            Err("it reverses a recent Account change")
        );
        assert_eq!(
            recommend(
                "budget-limited:702:Porta (702)",
                "keep",
                "Keep the daily budget of Porta and review its keywords.",
                "Porta lost 22% of its search impressions to budget after the budget went from $200.00 to $80.00."
            ),
            Ok("Porta lost 22% of its search impressions to budget.".into())
        );
    }

    #[test]
    fn a_budget_change_of_another_campaign_with_the_same_name_does_not_block_a_recommendation() {
        assert_eq!(
            recommend(
                "budget-limited:703:Porta (703)",
                "increase_budget",
                "Increase the daily budget of Porta.",
                "Porta lost 22% of its search impressions to budget."
            ),
            Ok("Porta lost 22% of its search impressions to budget.".into())
        );
    }
}
