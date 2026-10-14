use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Counts {
    pub impressions: f64,
    pub clicks: f64,
    pub conversions: f64,
    pub cost: f64,
    pub value: f64,
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
}
