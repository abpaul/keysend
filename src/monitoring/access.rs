use crate::billing::{Subscription, SubscriptionStatus};

pub const TRIAL_SECONDS: i64 = 14 * 86_400;
pub const AD_ACCOUNT_LIMIT: usize = 3;

pub const HISTORY_GRACE_SECONDS: i64 = 30 * 86_400;

pub fn history_open(started_at: i64, subscription: Option<&Subscription>, now: i64) -> bool {
    let access_ended = started_at
        .saturating_add(TRIAL_SECONDS)
        .max(subscription.map_or(0, |subscription| subscription.period_end));
    now < access_ended.saturating_add(HISTORY_GRACE_SECONDS)
}

pub fn trial_open(started_at: i64, now: i64) -> bool {
    now >= started_at && now < started_at.saturating_add(TRIAL_SECONDS)
}

pub fn subscription_open(subscription: Option<&Subscription>, now: i64) -> bool {
    subscription.is_some_and(|subscription| {
        subscription.status == SubscriptionStatus::Active && subscription.period_end > now
    })
}

pub fn payment_due(subscription: Option<&Subscription>) -> bool {
    subscription.is_some_and(|subscription| {
        matches!(
            subscription.status,
            SubscriptionStatus::PastDue | SubscriptionStatus::Unpaid
        )
    })
}
