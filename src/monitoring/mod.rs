pub mod access;
pub mod alerts;
pub mod assessment;
pub mod audit;
pub mod branding;
pub mod business;
pub mod campaign_settings;
pub mod campaign_text;
pub mod causes;
pub mod charts;
pub mod claims;
pub mod deep_check;
pub mod deep_view;
pub mod detection;
pub mod earlier;
pub mod economics;
pub mod email;
pub mod example;
pub mod explain;
pub mod export;
pub mod findings;
pub mod follow_up;
pub mod format;
pub mod metrics;
pub mod recommendation;
pub mod retention;
pub mod schedule;
pub mod tracking;
pub mod update;
pub mod view;
pub mod worker;

pub(crate) const UPDATES_SHOWN: u32 = 31;

#[cfg(test)]
mod eval;
#[cfg(test)]
mod tests;
