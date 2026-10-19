use serde::{Deserialize, Serialize};

use super::metrics::{last_settled_conversion_day, sum, CampaignDay};

pub const MIN_CONVERSIONS: f64 = 10.0;
/// Conversion tracking has stopped when this many settled days in a row have no conversion
/// while the Ad Account still gets `STOPPED_CLICKS` clicks in them.
pub const STOPPED_DAYS: i64 = 5;
pub const STOPPED_CLICKS: f64 = 100.0;

/// The settled days at the end of a period in which no conversion was recorded although the
/// Ad Account had clicks and, in the 30 days before them, conversions.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Stopped {
    pub since: i64,
    pub days: i64,
    pub clicks: f64,
    pub cost: f64,
    pub before: f64,
}

pub fn stopped(days: &[CampaignDay], end: i64) -> Option<Stopped> {
    let last = last_settled_conversion_day(end);
    let mut since = last + 1;
    while since > last - 60
        && sum(days, since - 1, since - 1, None)
            .conversions
            .unwrap_or_default()
            <= 0.0
    {
        since -= 1;
    }
    let quiet = sum(days, since, last, None);
    let before = sum(days, since - 30, since - 1, None)
        .conversions
        .unwrap_or_default();
    (last - since + 1 >= STOPPED_DAYS
        && quiet.clicks >= STOPPED_CLICKS
        && before >= MIN_CONVERSIONS)
        .then_some(Stopped {
            since,
            days: last - since + 1,
            clicks: quiet.clicks,
            cost: quiet.spend,
            before,
        })
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversionTracking {
    Working,
    TooFewResults,
    NotSetUp,
}

pub fn state(conversions: f64, conversion_actions: Option<u32>) -> ConversionTracking {
    if conversion_actions == Some(0) {
        ConversionTracking::NotSetUp
    } else if conversions < MIN_CONVERSIONS {
        ConversionTracking::TooFewResults
    } else {
        ConversionTracking::Working
    }
}

pub fn of_days(
    days: &[CampaignDay],
    end: i64,
    conversion_actions: Option<u32>,
    chosen: Option<bool>,
) -> (ConversionTracking, bool) {
    let conversions = sum(days, end - 29, end, None)
        .conversions
        .unwrap_or_default();
    let state = state(conversions, conversion_actions);
    (state, judge(state, chosen))
}

pub fn judge(state: ConversionTracking, chosen: Option<bool>) -> bool {
    chosen.unwrap_or(state == ConversionTracking::Working)
}

/// Conversions have jumped without the traffic behind them when the conversion rate of the
/// last 7 settled days is at least this many times that of the 28 days before, while clicks
/// per day stay within `SPIKE_CLICKS` of their level.
pub const SPIKE_RATE: f64 = 2.0;
pub const SPIKE_CLICKS: (f64, f64) = (0.75, 1.33);
const SPIKE_DAY_RATE: f64 = 1.6;

/// The settled days at the end of a period in which conversions per click jumped while clicks
/// held: a sign of conversions counted twice rather than of more customers.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Spike {
    pub since: i64,
    pub days: i64,
    pub rate: f64,
    pub usual: f64,
}

pub fn spike(days: &[CampaignDay], end: i64) -> Option<Spike> {
    let last = last_settled_conversion_day(end);
    let rate = |metrics: &super::metrics::Metrics| {
        (metrics.clicks > 0.0).then(|| metrics.conversions.unwrap_or_default() / metrics.clicks)
    };
    let recent = sum(days, last - 6, last, None);
    let usual_days = sum(days, last - 34, last - 7, None);
    let (now, usual) = (rate(&recent)?, rate(&usual_days)?);
    let clicks = (recent.clicks / 7.0) / (usual_days.clicks / 28.0);
    if !(now >= usual * SPIKE_RATE
        && recent.conversions.unwrap_or_default() >= MIN_CONVERSIONS
        && (SPIKE_CLICKS.0..=SPIKE_CLICKS.1).contains(&clicks))
    {
        return None;
    }
    let mut since = last + 1;
    while since > last - 27
        && rate(&sum(days, since - 1, since - 1, None))
            .is_some_and(|day| day >= usual * SPIKE_DAY_RATE)
    {
        since -= 1;
    }
    Some(Spike {
        since,
        days: last - since + 1,
        rate: now,
        usual,
    })
}

/// The last day of each conversion lag bucket of Google Ads.
const LAG_DAYS: [(&str, i64); 19] = [
    ("LESS_THAN_ONE_DAY", 1),
    ("ONE_TO_TWO_DAYS", 2),
    ("TWO_TO_THREE_DAYS", 3),
    ("THREE_TO_FOUR_DAYS", 4),
    ("FOUR_TO_FIVE_DAYS", 5),
    ("FIVE_TO_SIX_DAYS", 6),
    ("SIX_TO_SEVEN_DAYS", 7),
    ("SEVEN_TO_EIGHT_DAYS", 8),
    ("EIGHT_TO_NINE_DAYS", 9),
    ("NINE_TO_TEN_DAYS", 10),
    ("TEN_TO_ELEVEN_DAYS", 11),
    ("ELEVEN_TO_TWELVE_DAYS", 12),
    ("TWELVE_TO_THIRTEEN_DAYS", 13),
    ("THIRTEEN_TO_FOURTEEN_DAYS", 14),
    ("FOURTEEN_TO_TWENTY_ONE_DAYS", 21),
    ("TWENTY_ONE_TO_THIRTY_DAYS", 30),
    ("THIRTY_TO_FORTY_FIVE_DAYS", 45),
    ("FORTY_FIVE_TO_SIXTY_DAYS", 60),
    ("SIXTY_TO_NINETY_DAYS", 90),
];

/// The lag counts only from this share of conversions not yet recorded in the period.
pub const LAG_SHARE: f64 = 0.03;

/// How late conversions arrive, from the conversion lag buckets of the Ad Account.
#[derive(Clone, Debug, PartialEq)]
pub struct Lag {
    /// The share of conversions that arrive more than a week after the click.
    pub late: f64,
    /// The share of the conversions of the last 30 days and of the 30 days before them that
    /// has probably not been recorded yet.
    pub missing: (f64, f64),
}

pub fn lag(buckets: &[(String, f64)]) -> Option<Lag> {
    let known = buckets
        .iter()
        .filter_map(|(bucket, conversions)| {
            LAG_DAYS
                .iter()
                .find(|(name, _)| name == bucket)
                .map(|(_, days)| (*days, *conversions))
        })
        .collect::<Vec<_>>();
    let total = known
        .iter()
        .map(|(_, conversions)| conversions)
        .sum::<f64>();
    if total < MIN_CONVERSIONS {
        return None;
    }
    let arrived = |age: i64| {
        known
            .iter()
            .filter(|(days, _)| *days <= age)
            .map(|(_, conversions)| conversions)
            .sum::<f64>()
            / total
    };
    let missing = |ages: std::ops::RangeInclusive<i64>| {
        1.0 - ages.clone().map(arrived).sum::<f64>() / ages.count() as f64
    };
    Some(Lag {
        late: 1.0 - arrived(7),
        missing: (missing(1..=30), missing(31..=60)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitoring::metrics::Metrics;

    fn day(day: i64, clicks: f64, conversions: f64) -> CampaignDay {
        CampaignDay {
            day,
            campaign_id: 1,
            metrics: Metrics {
                spend: clicks,
                clicks,
                impressions: clicks * 10.0,
                conversions: Some(conversions),
                conversion_value: None,
            },
        }
    }

    #[test]
    fn conversions_that_stop_while_clicks_continue_are_a_stop_of_tracking() {
        let end = 100;
        let days = (0..=end)
            .map(|index| day(index, 60.0, if index > end - 12 { 0.0 } else { 3.0 }))
            .collect::<Vec<_>>();
        assert_eq!(
            stopped(&days, end),
            Some(Stopped {
                since: 89,
                days: 11,
                clicks: 660.0,
                cost: 660.0,
                before: 90.0,
            })
        );
        let quiet = (0..=end)
            .map(|index| day(index, 5.0, if index > end - 12 { 0.0 } else { 3.0 }))
            .collect::<Vec<_>>();
        assert_eq!(stopped(&quiet, end), None);
        let short = (0..=end)
            .map(|index| day(index, 60.0, if index > end - 4 { 0.0 } else { 3.0 }))
            .collect::<Vec<_>>();
        assert_eq!(stopped(&short, end), None);
    }

    #[test]
    fn conversions_that_jump_while_clicks_hold_are_a_spike_and_real_growth_is_not() {
        let end = 100;
        let jump = |clicks_after: f64| {
            (0..=end)
                .map(|index| {
                    if index > end - 10 {
                        day(index, clicks_after, 7.2)
                    } else {
                        day(index, 60.0, 3.0)
                    }
                })
                .collect::<Vec<_>>()
        };
        let found = spike(&jump(60.0), end).unwrap();
        assert_eq!((found.since, found.days), (91, 9));
        assert_eq!(spike(&jump(150.0), end), None);
    }

    #[test]
    fn the_lag_buckets_give_the_late_share_and_what_the_last_30_days_still_miss() {
        let buckets = [
            ("LESS_THAN_ONE_DAY", 50.0),
            ("SIX_TO_SEVEN_DAYS", 20.0),
            ("FOURTEEN_TO_TWENTY_ONE_DAYS", 30.0),
            ("UNKNOWN", 40.0),
        ]
        .map(|(bucket, conversions)| (bucket.to_owned(), conversions));
        let lag = lag(&buckets).unwrap();
        assert!((lag.late - 0.3).abs() < 1e-9);
        assert!((lag.missing.0 - (0.5 * 6.0 + 0.3 * 14.0) / 30.0).abs() < 1e-9);
        assert_eq!(lag.missing.1, 0.0);
    }
}
