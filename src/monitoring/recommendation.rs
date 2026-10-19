use serde::{Deserialize, Serialize};
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
pub const ACTION_LENGTH: usize = 200;
pub const REASON_LENGTH: usize = 300;
pub const MEASURE_LENGTH: usize = 200;

pub(crate) struct Proposal {
    pub finding: String,
    pub intent: String,
    pub subject: String,
    pub action: String,
    pub reason: String,
    pub evidence: String,
}

fn contains_any(text: &str, parts: &[&str]) -> bool {
    parts.iter().any(|part| text.contains(part))
}

/// What a Recommendation may do about each rule of Finding: the intents that it permits and
/// the levers that a consultant usually reaches for.
/// How strongly the evidence of its Finding supports a Recommendation.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    Low,
    #[default]
    Medium,
    High,
}

impl Confidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    fn lower(self) -> Self {
        match self {
            Self::High => Self::Medium,
            _ => Self::Low,
        }
    }
}

/// What a Recommendation may do about each rule of Finding: the intents that it permits, the
/// levers that a consultant usually reaches for, how strongly the rule's evidence supports an
/// action, and what that evidence cannot tell.
pub(crate) struct Rule {
    pub intents: &'static [&'static str],
    pub levers: &'static str,
    pub confidence: Confidence,
    pub uncertainty: &'static str,
}

const CUT: &[&str] = &["reduce_spend", "pause", "improve", "investigate"];

pub(crate) fn rule(finding: &str) -> Rule {
    use Confidence::{High, Low, Medium};
    let (intents, levers, confidence, uncertainty): (&'static [&'static str], _, _, _) =
        match finding.split(':').next().unwrap_or_default() {
            "search-terms-no-conversions" | "keywords-no-conversions" => (
                &["exclude", "pause", "reduce_spend", "investigate"],
                "negative keywords, a narrower match type",
                High,
                "a term can still convert later, so a term close to the business deserves a look before it is excluded",
            ),
            "traffic-without-results" => (
                &["exclude", "improve", "reduce_spend", "investigate"],
                "the search terms behind the extra clicks, negative keywords, a narrower match type, the landing page of the new traffic",
                Medium,
                "the comparison shows that the extra clicks brought no more conversions, not which searches or pages brought them",
            ),
            "click-cost-rising" => (
                &["investigate", "improve", "reduce_spend", "keep"],
                "auction insights and impression share to see whether competition rose, the bid strategy target, a maximum cost per click",
                Medium,
                "the data shows that clicks cost more, not whether competition or bidding made them cost more",
            ),
            "location-cost-high" => (
                &["investigate", "reduce_spend", "improve"],
                "the quality of the leads from that location, a location bid adjustment, a separate campaign for that location",
                Medium,
                "Google Ads does not show whether the leads of one location are worth more than those of another",
            ),
            "budget-limited" => (
                &["increase_budget", "investigate", "keep"],
                "budget moved from a weaker campaign, a higher daily budget with a cost per conversion limit",
                Medium,
                "the extra clicks that a higher budget buys usually cost more than the current ones",
            ),
            "landing-page-low-rate" => (
                &["improve", "investigate"],
                "sending the traffic to a page that converts better, a test of the page against it",
                Medium,
                "pages can serve searches with different intent, so a lower rate is not always the page's fault",
            ),
            "ads-poor-strength" => (
                &["improve", "investigate"],
                "more distinct headlines and descriptions that match the keywords",
                Low,
                "ad strength is a rating of Google, not a result",
            ),
            "campaign-below-break-even" => (
                CUT,
                "a target return on ad spend at or above the break-even return, product groups split by return, budget moved to campaigns above break-even",
                Medium,
                "the break-even rests on the margin in the Ad Account profile",
            ),
            "campaign-above-break-even" => (
                CUT,
                "a target cost per conversion at or below the break-even cost, tighter keywords, a higher share of conversions becoming customers",
                Medium,
                "the break-even rests on the value, close rate and margin in the Ad Account profile",
            ),
            "campaign-return-low" => (
                CUT,
                "product groups split by return, a target return on ad spend, budget moved to campaigns with a higher return",
                High,
                "without a margin in the Ad Account profile, the return that pays is not known",
            ),
            "campaign-click-cost-high" => (
                CUT,
                "a maximum cost per click, the keywords with the dearest clicks",
                Medium,
                "results are not judged here, so a dear click may still be a good one",
            ),
            "tracking-not-set-up" | "tracking-no-results" | "tracking-stopped" | "tracking-spike" => {
                (&[], "", High, "")
            }
            _ => (
                CUT,
                "budget moved to a campaign with a lower cost per conversion, the bid strategy target, the search terms",
                High,
                "conversions are counted the same way in every campaign, but their value may differ",
            ),
        };
    Rule {
        intents,
        levers,
        confidence,
        uncertainty,
    }
}

/// The confidence of a Recommendation: that of its rule, one level lower when the Ad Account
/// records too few conversions to be sure or when late conversions are still missing.
pub(crate) fn confidence(finding: &str, settled: bool) -> Confidence {
    let base = rule(finding).confidence;
    if settled || finding.starts_with("tracking") || finding.starts_with("ads-poor-strength") {
        base
    } else {
        base.lower()
    }
}

fn permitted(finding: &str) -> &'static [&'static str] {
    rule(finding).intents
}

/// An action names a subject by its full name, by the path of a landing page, or by the first
/// headline of an ad.
fn named(action: &str, subject: &str) -> bool {
    let subject = subject.to_lowercase();
    let path = reqwest::Url::parse(&subject)
        .ok()
        .map(|url| url.path().trim_end_matches('/').to_owned())
        .filter(|path| path.len() > 1);
    let headline = subject.split_once(" | ").map(|(first, _)| first.to_owned());
    std::iter::once(subject)
        .chain(path)
        .chain(headline)
        .any(|form| action.contains(&form))
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
    let subject = finding
        .subjects
        .iter()
        .find(|subject| named(&lower, subject))
        .cloned();
    if !finding.subjects.is_empty() && subject.is_none() {
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
        intent: intent.into(),
        subject: subject.unwrap_or_default(),
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

    #[test]
    fn an_action_names_a_landing_page_by_its_path_and_an_ad_by_its_first_headline() {
        assert!(named(
            "send emergency clicks from /offers back to /emergency.",
            "https://northsideplumbing.example/offers"
        ));
        assert!(named(
            "rewrite the spring offer ad around emergency call outs.",
            "Spring Offer | 20% Off Plumbing | Book Online"
        ));
        assert!(!named("improve the home page.", "https://example.com/"));
        assert!(!named("rewrite the offer ad.", "Spring Offer | 20% Off"));
    }
}
