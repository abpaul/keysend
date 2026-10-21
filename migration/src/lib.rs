#![allow(elided_lifetimes_in_paths)]
pub use sea_orm_migration::prelude::*;

mod m20261011_000001_identity;
mod m20261011_000002_reports;
mod m20261011_000003_billing;
mod m20261011_000004_google_reports;
mod m20261012_000005_account_history;
mod m20261012_000006_analysis;

mod m20261012_000007_monitoring;
mod m20261012_000008_subscription_price;
mod m20261012_000009_drop_reports;
mod m20261012_000010_trial_history;
mod m20261012_000011_breakdowns;
mod m20261012_000012_campaign_settings;
mod m20261013_000013_user_language;
mod m20261013_000014_translations;
mod m20261013_000015_sign_up_source;
mod m20261014_000016_results_sentence;
mod m20261019_000017_profile_economics;

mod m20261020_000018_admin;
mod m20261020_000019_measurement;

mod m20261020_000020_investigations;
mod m20261021_000021_optimization;
mod m20261021_000022_shadow;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20261011_000001_identity::Migration),
            Box::new(m20261011_000002_reports::Migration),
            Box::new(m20261011_000003_billing::Migration),
            Box::new(m20261011_000004_google_reports::Migration),
            Box::new(m20261012_000005_account_history::Migration),
            Box::new(m20261012_000006_analysis::Migration),
            Box::new(m20261012_000007_monitoring::Migration),
            Box::new(m20261012_000008_subscription_price::Migration),
            Box::new(m20261012_000009_drop_reports::Migration),
            Box::new(m20261012_000010_trial_history::Migration),
            Box::new(m20261012_000011_breakdowns::Migration),
            Box::new(m20261012_000012_campaign_settings::Migration),
            Box::new(m20261013_000013_user_language::Migration),
            Box::new(m20261013_000014_translations::Migration),
            Box::new(m20261013_000015_sign_up_source::Migration),
            Box::new(m20261014_000016_results_sentence::Migration),
            Box::new(m20261019_000017_profile_economics::Migration),
            Box::new(m20261020_000018_admin::Migration),
            Box::new(m20261020_000019_measurement::Migration),
            Box::new(m20261020_000020_investigations::Migration),
            Box::new(m20261021_000021_optimization::Migration),
            Box::new(m20261021_000022_shadow::Migration),
            // inject-above (do not remove this comment)
        ]
    }
}
