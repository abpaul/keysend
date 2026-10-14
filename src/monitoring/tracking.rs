use serde::{Deserialize, Serialize};

use super::metrics::{sum, CampaignDay};

pub const MIN_CONVERSIONS: f64 = 10.0;

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
