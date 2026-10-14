use loco_rs::Result;
use sea_orm::TransactionTrait;

use super::{monitoring::AccountSettings, profile::Profile, WorkspaceStore};

pub struct AdAccountSettings<'a> {
    pub profile: &'a Profile,
    pub branding_name: &'a str,
    pub accent_colour: &'a str,
    pub logo: Option<&'a [u8]>,
    pub monitoring: &'a AccountSettings,
}

impl WorkspaceStore<'_> {
    pub async fn save_ad_account_settings(
        &self,
        ad_account: &str,
        settings: &AdAccountSettings<'_>,
    ) -> Result<bool> {
        let txn = self.db.begin().await?;
        let saved = self
            .save_profile_on(&txn, ad_account, settings.profile)
            .await?
            && self
                .save_branding_on(
                    &txn,
                    ad_account,
                    settings.branding_name,
                    settings.accent_colour,
                )
                .await?
            && match settings.logo {
                Some(logo) => self.save_logo_on(&txn, ad_account, logo).await?,
                None => true,
            };
        if !saved {
            return Ok(false);
        }
        self.save_monitoring_settings_on(&txn, ad_account, settings.monitoring)
            .await?;
        txn.commit().await?;
        Ok(true)
    }
}
