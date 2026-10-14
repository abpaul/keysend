use std::collections::BTreeMap;

use serde::Serialize;

use super::tracking::ConversionTracking;
use crate::workspace::monitoring::Recommendation;

pub const NOTE_LENGTH: usize = 500;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub enum Choice {
    #[default]
    Inherit,
    Yes,
    No,
}

impl Choice {
    pub fn parse(text: &str, yes: &str, no: &str) -> Option<Self> {
        match text {
            "inherit" => Some(Self::Inherit),
            text if text == yes => Some(Self::Yes),
            text if text == no => Some(Self::No),
            _ => None,
        }
    }

    pub fn word(self, yes: &'static str, no: &'static str) -> &'static str {
        match self {
            Self::Inherit => "inherit",
            Self::Yes => yes,
            Self::No => no,
        }
    }

    pub fn from_flag(flag: Option<bool>) -> Self {
        match flag {
            None => Self::Inherit,
            Some(true) => Self::Yes,
            Some(false) => Self::No,
        }
    }

    pub fn flag(self) -> Option<bool> {
        match self {
            Self::Inherit => None,
            Self::Yes => Some(true),
            Self::No => Some(false),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CampaignSettings {
    pub target_cost: Option<f64>,
    pub note: Option<String>,
    pub alerts: Choice,
    pub judge_by_conversions: Choice,
}

impl CampaignSettings {
    pub fn valid(&self) -> bool {
        self.target_cost.is_none_or(|cost| cost > 0.0 && cost < 1e9)
            && self
                .note
                .as_ref()
                .is_none_or(|note| (1..=NOTE_LENGTH).contains(&note.chars().count()))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AccountDefaults {
    pub state: ConversionTracking,
    pub judged: bool,
    pub target_cost: Option<f64>,
    pub alerts: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetFrom {
    AdAccount,
    Campaign,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Target {
    pub cost: f64,
    pub from: TargetFrom,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Effective<'a> {
    pub target: Option<Target>,
    pub note: Option<&'a str>,
    pub alerts: bool,
    pub judged: bool,
}

pub fn alerts_on(
    account: bool,
    own: &BTreeMap<i64, CampaignSettings>,
    campaign: Option<i64>,
) -> bool {
    campaign
        .and_then(|id| own.get(&id))
        .map_or(account, |settings| match settings.alerts {
            Choice::Inherit => account,
            Choice::Yes => true,
            Choice::No => false,
        })
}

#[derive(Clone, Debug)]
pub struct Resolved {
    account: AccountDefaults,
    own: BTreeMap<i64, CampaignSettings>,
}

impl Resolved {
    pub fn new(account: AccountDefaults, own: BTreeMap<i64, CampaignSettings>) -> Self {
        Self { account, own }
    }

    pub fn account(&self) -> &AccountDefaults {
        &self.account
    }

    pub fn own(&self, campaign: i64) -> Option<&CampaignSettings> {
        self.own.get(&campaign)
    }

    pub fn shows(&self, campaign: i64, text: &str) -> bool {
        self.effective(campaign).judged || !mentions_conversions(text)
    }

    pub fn shows_recommendation(&self, recommendation: &Recommendation) -> bool {
        recommendation.campaign_id.is_none_or(|campaign| {
            self.shows(
                campaign,
                &format!(
                    "{} {} {}",
                    recommendation.action, recommendation.reason, recommendation.evidence
                ),
            )
        })
    }

    pub fn effective(&self, campaign: i64) -> Effective<'_> {
        let account = &self.account;
        let own = self.own.get(&campaign);
        Effective {
            target: own
                .and_then(|own| own.target_cost)
                .map(|cost| Target {
                    cost,
                    from: TargetFrom::Campaign,
                })
                .or(account
                    .target_cost
                    .filter(|cost| *cost > 0.0)
                    .map(|cost| Target {
                        cost,
                        from: TargetFrom::AdAccount,
                    })),
            note: own.and_then(|own| own.note.as_deref()),
            alerts: alerts_on(account.alerts, &self.own, Some(campaign)),
            judged: match own.map(|own| own.judge_by_conversions) {
                None | Some(Choice::Inherit) => account.judged,
                Some(Choice::No) => false,
                Some(Choice::Yes) => account.judged || account.state == ConversionTracking::Working,
            },
        }
    }
}

pub trait Rules {
    fn judged(&self) -> bool;
    fn campaign_judged(&self, campaign: i64) -> bool;
    fn note(&self, campaign: i64) -> Option<&str>;
}

#[derive(Clone, Copy)]
pub struct AllFollowAccount {
    pub judged: bool,
}

impl Rules for AllFollowAccount {
    fn judged(&self) -> bool {
        self.judged
    }

    fn campaign_judged(&self, _: i64) -> bool {
        self.judged
    }

    fn note(&self, _: i64) -> Option<&str> {
        None
    }
}

impl Rules for Resolved {
    fn judged(&self) -> bool {
        self.account.judged
    }

    fn campaign_judged(&self, campaign: i64) -> bool {
        self.effective(campaign).judged
    }

    fn note(&self, campaign: i64) -> Option<&str> {
        self.effective(campaign).note
    }
}

impl<T: Rules + ?Sized> Rules for &T {
    fn judged(&self) -> bool {
        (**self).judged()
    }

    fn campaign_judged(&self, campaign: i64) -> bool {
        (**self).campaign_judged(campaign)
    }

    fn note(&self, campaign: i64) -> Option<&str> {
        (**self).note(campaign)
    }
}

pub fn mentions_conversions(text: &str) -> bool {
    text.to_lowercase().contains("conversion")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(state: ConversionTracking, judged: bool) -> AccountDefaults {
        AccountDefaults {
            state,
            judged,
            target_cost: Some(45.0),
            alerts: true,
        }
    }

    fn resolved(account: AccountDefaults, own: CampaignSettings) -> Resolved {
        Resolved::new(account, BTreeMap::from([(701, own)]))
    }

    #[test]
    fn a_campaign_without_settings_follows_the_ad_account() {
        let rules = Resolved::new(account(ConversionTracking::Working, true), BTreeMap::new());
        let effective = rules.effective(701);
        assert_eq!(
            effective.target,
            Some(Target {
                cost: 45.0,
                from: TargetFrom::AdAccount
            })
        );
        assert_eq!(effective.note, None);
        assert!(effective.alerts);
        assert!(effective.judged);
    }

    #[test]
    fn own_values_replace_the_values_of_the_ad_account() {
        let rules = resolved(
            account(ConversionTracking::Working, true),
            CampaignSettings {
                target_cost: Some(60.0),
                note: Some("Closed on Sundays.".into()),
                alerts: Choice::No,
                judge_by_conversions: Choice::No,
            },
        );
        let effective = rules.effective(701);
        assert_eq!(
            effective.target,
            Some(Target {
                cost: 60.0,
                from: TargetFrom::Campaign
            })
        );
        assert_eq!(effective.note, Some("Closed on Sundays."));
        assert!(!effective.alerts);
        assert!(!effective.judged);
        assert!(rules.effective(702).judged);
    }

    #[test]
    fn yes_cannot_override_conversion_tracking_that_does_not_work() {
        let yes = CampaignSettings {
            judge_by_conversions: Choice::Yes,
            ..Default::default()
        };
        for state in [
            ConversionTracking::TooFewResults,
            ConversionTracking::NotSetUp,
        ] {
            assert!(
                !resolved(account(state, false), yes.clone())
                    .effective(701)
                    .judged
            );
            assert!(
                resolved(account(state, true), yes.clone())
                    .effective(701)
                    .judged
            );
        }
        assert!(
            resolved(account(ConversionTracking::Working, false), yes)
                .effective(701)
                .judged
        );
    }

    #[test]
    fn a_campaign_can_turn_alerts_on_for_an_ad_account_that_has_them_off() {
        let mut off = account(ConversionTracking::Working, true);
        off.alerts = false;
        let own = CampaignSettings {
            alerts: Choice::Yes,
            ..Default::default()
        };
        let rules = resolved(off, own);
        assert!(rules.effective(701).alerts);
        assert!(!rules.effective(702).alerts);
    }

    #[test]
    fn a_target_of_zero_is_not_valid_and_a_long_note_is_not_valid() {
        let target = |cost| CampaignSettings {
            target_cost: Some(cost),
            ..Default::default()
        };
        assert!(target(0.01).valid());
        assert!(!target(0.0).valid());
        assert!(!target(-1.0).valid());
        let note = |length: usize| CampaignSettings {
            note: Some("a".repeat(length)),
            ..Default::default()
        };
        assert!(note(500).valid());
        assert!(!note(501).valid());
        assert!(!note(0).valid());
    }
}
