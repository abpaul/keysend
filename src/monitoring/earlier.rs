//! The earlier Recommendations of an Ad Account: what followed each one, and whether a new
//! action would repeat one. What followed is what came after, never proof of an effect.

use serde_json::{json, Value};

use super::metrics::{
    english_count, english_money, english_number, last_settled_conversion_day, sum, CampaignDay,
};
use crate::{
    language::english,
    workspace::{
        history::date_of,
        monitoring::{Recommendation, RecommendationState},
    },
};

/// What followed a Recommendation is shown once this many settled days have passed.
pub const FOLLOW_DAYS: i64 = 14;
/// A new action repeats an earlier one while the earlier one is open or closed for less.
pub const REPEAT_DAYS: i64 = 90;
const BEFORE_DAYS: i64 = 30;

#[derive(Clone, Debug, PartialEq)]
pub struct Earlier {
    pub recommendation: Recommendation,
    pub opened_day: i64,
    pub closed_day: Option<i64>,
    pub still_detected: bool,
}

fn date(day: i64) -> String {
    date_of(day).map_or_else(String::new, |date| english().written.format.day_month(date))
}

fn state(state: &RecommendationState) -> &'static str {
    match state {
        RecommendationState::Open => "open",
        RecommendationState::Done => "marked done",
        RecommendationState::NotUseful => "marked not useful",
        RecommendationState::Resolved => "resolved: its Finding is no longer detected",
    }
}

impl Earlier {
    fn current(&self, day: i64) -> bool {
        self.recommendation.state == RecommendationState::Open
            || self
                .closed_day
                .is_some_and(|closed| day - closed < REPEAT_DAYS)
    }

    /// Whether an action with this intent on this subject would say again what this one said.
    pub fn repeats(&self, day: i64, campaign: Option<i64>, subject: &str, intent: &str) -> bool {
        let earlier = &self.recommendation;
        self.current(day)
            && earlier.campaign_id == campaign
            && (earlier.subject.is_empty() || subject.is_empty() || earlier.subject == subject)
            && (earlier.intent.is_empty() || earlier.intent == intent)
    }

    /// The Campaign of the Recommendation in the settled days since it opened, against the 30
    /// days before, in a sentence that names no effect.
    pub fn followed(
        &self,
        days: &[CampaignDay],
        end: i64,
        name: &str,
        currency: &str,
    ) -> Option<String> {
        let campaign = self.recommendation.campaign_id?;
        let last = last_settled_conversion_day(end);
        let since = last - self.opened_day + 1;
        if since < FOLLOW_DAYS {
            return None;
        }
        let after = sum(days, self.opened_day, last, Some(campaign));
        let before = sum(
            days,
            self.opened_day - BEFORE_DAYS,
            self.opened_day - 1,
            Some(campaign),
        );
        let result = |metrics: &super::metrics::Metrics| {
            let conversions = metrics.conversions.unwrap_or_default();
            let value = metrics.conversion_value.unwrap_or_default();
            if value > 0.0 && value >= metrics.spend {
                format!(
                    "a return on ad spend of {} on {} spend",
                    english_number(value / metrics.spend.max(f64::EPSILON), 2),
                    english_money(metrics.spend, currency)
                )
            } else if conversions > 0.0 {
                format!(
                    "{} conversions at {} each",
                    english_count(conversions),
                    english_money(metrics.spend / conversions, currency)
                )
            } else {
                format!(
                    "0 conversions for {} spend",
                    english_money(metrics.spend, currency)
                )
            }
        };
        Some(format!(
            "In the {since} settled days since {}, {name} had {}, against {} in the {BEFORE_DAYS} days before. Other changes in the same days may also have played a part.",
            date(self.opened_day),
            result(&after),
            result(&before)
        ))
    }

    pub fn facts(&self, days: &[CampaignDay], end: i64, name: &str, currency: &str) -> Value {
        let earlier = &self.recommendation;
        json!({
            "key": earlier.finding_key,
            "action": earlier.action,
            "subject": earlier.subject,
            "state": state(&earlier.state),
            "opened": date(self.opened_day),
            "closed": self.closed_day.map(date),
            "finding_still_detected": self.still_detected,
            "may_be_proposed_again": !self.current(end + 1),
            "what_followed": self.followed(days, end, name, currency),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitoring::{metrics::Metrics, recommendation::Confidence};

    fn earlier(state: RecommendationState, opened_day: i64, closed_day: Option<i64>) -> Earlier {
        Earlier {
            recommendation: Recommendation {
                finding_key: "search-terms-no-conversions:31".into(),
                action: "Add negative keywords in Search - Drain Cleaning.".into(),
                reason: String::new(),
                evidence: String::new(),
                measure: String::new(),
                intent: "exclude".into(),
                subject: "plumber salary".into(),
                confidence: Confidence::High,
                campaign_id: Some(31),
                state,
            },
            opened_day,
            closed_day,
            still_detected: false,
        }
    }

    #[test]
    fn what_followed_compares_the_settled_days_since_with_the_month_before_and_names_no_effect() {
        let days = (0..=200)
            .map(|day| CampaignDay {
                day,
                campaign_id: 31,
                metrics: Metrics {
                    spend: 100.0,
                    clicks: 50.0,
                    impressions: 500.0,
                    conversions: Some(if day >= 155 { 4.0 } else { 2.5 }),
                    conversion_value: Some(0.0),
                },
            })
            .collect::<Vec<_>>();
        let done = earlier(RecommendationState::Done, 155, Some(160));
        assert_eq!(
            done.followed(&days, 200, "Search - Drain Cleaning", "USD").unwrap(),
            "In the 45 settled days since 5 June, Search - Drain Cleaning had 180 conversions at $25.00 each, against 75 conversions at $40.00 each in the 30 days before. Other changes in the same days may also have played a part."
        );
        assert_eq!(
            earlier(RecommendationState::Done, 190, None).followed(&days, 200, "x", "USD"),
            None
        );
    }

    #[test]
    fn an_open_or_recently_closed_recommendation_is_not_proposed_again_for_the_same_subject_and_intent(
    ) {
        let open = earlier(RecommendationState::Open, 100, None);
        assert!(open.repeats(200, Some(31), "plumber salary", "exclude"));
        assert!(!open.repeats(200, Some(31), "plumber salary", "reduce_spend"));
        assert!(!open.repeats(200, Some(32), "plumber salary", "exclude"));
        let old = earlier(RecommendationState::Done, 50, Some(100));
        assert!(!old.repeats(200, Some(31), "plumber salary", "exclude"));
        assert!(
            earlier(RecommendationState::NotUseful, 150, Some(150)).repeats(
                200,
                Some(31),
                "",
                "exclude"
            )
        );
    }
}
