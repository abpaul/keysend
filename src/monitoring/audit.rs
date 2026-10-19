use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Counts {
    pub impressions: f64,
    pub clicks: f64,
    pub conversions: f64,
    pub cost: f64,
    pub value: f64,
}

impl Counts {
    /// Results are judged by value when the conversion value is at least the spend, as for
    /// sales; a lead value of one per conversion leaves them judged by cost per conversion.
    pub fn judged_by_value(&self) -> bool {
        self.value > 0.0 && self.value >= self.cost
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Data {
    pub currency: String,
    pub reporting: Counts,
    #[serde(default)]
    pub audit: Audit,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct AuditRow {
    pub name: String,
    #[serde(default)]
    pub id: Option<i64>,
    #[serde(default)]
    pub counts: Counts,
    #[serde(default)]
    pub lost_to_budget: Option<f64>,
    #[serde(default)]
    pub strength: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<Counts>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct AccountChange {
    pub date: String,
    pub campaign: String,
    #[serde(default)]
    pub campaign_id: Option<i64>,
    pub text: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Audit {
    #[serde(default)]
    pub conversion_actions: Option<u32>,
    #[serde(default)]
    pub campaigns: Vec<AuditRow>,
    #[serde(default)]
    pub search_terms: Vec<AuditRow>,
    #[serde(default)]
    pub keywords: Vec<AuditRow>,
    #[serde(default)]
    pub landing_pages: Vec<AuditRow>,
    #[serde(default)]
    pub ads: Vec<AuditRow>,
    #[serde(default)]
    pub changes: Vec<AccountChange>,
    /// The Ad Account in the 30 days before the period of the Deep check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<Counts>,
    /// The same 30 days 52 weeks earlier, then the 30 days before them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_year: Option<[Counts; 2]>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locations: Vec<AuditRow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped: Option<super::tracking::Stopped>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spike: Option<super::tracking::Spike>,
    /// The conversions of each conversion lag bucket of Google Ads.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lag: Vec<(String, f64)>,
    #[serde(default, skip_serializing_if = "super::economics::Economics::is_empty")]
    pub economics: super::economics::Economics,
}
