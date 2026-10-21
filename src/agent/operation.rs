//! The operations that the model may choose. Each is a business question with typed, bounded
//! arguments; code decides which rows of the store answer it. There is no operation that
//! writes, takes a query, or names a Workspace.

use serde::{Deserialize, Serialize};

use super::Handle;

/// The most rows that any operation returns. Rows beyond it are counted, not sent.
pub const MAX_ROWS: u8 = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Period {
    #[serde(rename = "last_7_days")]
    Last7Days,
    #[serde(rename = "last_30_days")]
    Last30Days,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Baseline {
    /// The same number of days just before the period.
    PreviousPeriod,
    /// The same days one year earlier.
    LastYear,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CampaignOrder {
    Spend,
    SpendChange,
    ConversionChange,
    CostPerConversionChange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchTermFocus {
    TopSpend,
    SpendWithoutConversions,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Segment {
    Device,
    Location,
    Hour,
    Demographic,
}

/// A row limit that the model chose, bounded by [`MAX_ROWS`] when it is parsed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct Rows(u8);

impl Rows {
    pub fn get(self) -> usize {
        usize::from(self.0)
    }
}

impl TryFrom<u8> for Rows {
    type Error = &'static str;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        if (1..=MAX_ROWS).contains(&value) {
            Ok(Self(value))
        } else {
            Err("row limit out of range")
        }
    }
}

impl From<Rows> for u8 {
    fn from(rows: Rows) -> Self {
        rows.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    /// Every Ad Account of the investigation, with its main Metrics, Conversion tracking and
    /// its active Findings by spend, for questions across clients.
    RankAccounts { period: Period },
    /// One Ad Account: Metrics of the period, Conversion tracking, profile goal and target,
    /// and the active Findings.
    AccountOverview {
        #[serde(deserialize_with = "account_handle")]
        account: Handle,
        period: Period,
    },
    /// The Metrics against the previous period and the same days last year, with movements
    /// outside normal variation, their Contributions and the Cause candidates that code found.
    ComparePeriods {
        #[serde(deserialize_with = "account_handle")]
        account: Handle,
        period: Period,
    },
    /// The Campaigns of one Ad Account against the previous period, ranked and cut.
    CompareCampaigns {
        #[serde(deserialize_with = "account_handle")]
        account: Handle,
        period: Period,
        order: CampaignOrder,
        rows: Rows,
    },
    /// Conversion tracking: its state, enabled actions, conversions by action and week, the
    /// conversion delay and the days not yet settled, stops and spikes.
    InvestigateConversions {
        #[serde(deserialize_with = "account_handle")]
        account: Handle,
    },
    /// Search terms of the latest Deep check, for one Campaign or all.
    InvestigateSearchTerms {
        #[serde(deserialize_with = "account_handle")]
        account: Handle,
        #[serde(deserialize_with = "required_campaign")]
        campaign: Option<i64>,
        focus: SearchTermFocus,
        rows: Rows,
    },
    /// One segment of the Ad Account: devices from the Account history, locations, hours and
    /// demographics from the latest Deep check.
    InvestigateSegments {
        #[serde(deserialize_with = "account_handle")]
        account: Handle,
        segment: Segment,
    },
    /// The economics of the Ad Account profile and what code derives from them, with the
    /// inputs that are missing.
    ReviewEconomics {
        #[serde(deserialize_with = "account_handle")]
        account: Handle,
    },
    /// The latest Assessment, active and resolved Findings, Recommendations with their state
    /// and what followed them, and the Account changes with their Follow-ups.
    ReviewHistory {
        #[serde(deserialize_with = "account_handle")]
        account: Handle,
    },
    /// The verified Business context, with its sources and research date.
    BusinessContext {
        #[serde(deserialize_with = "account_handle")]
        account: Handle,
    },
}

fn required_campaign<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<i64>, D::Error> {
    Option::<i64>::deserialize(deserializer)
}

fn account_handle<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Handle, D::Error> {
    let value = String::deserialize(deserializer)?;
    if value.len() != 2 {
        return Err(serde::de::Error::custom("unknown Ad Account"));
    }
    Handle::try_from(value).map_err(serde::de::Error::custom)
}

impl Operation {
    pub fn account(&self) -> Option<Handle> {
        match self {
            Self::RankAccounts { .. } => None,
            Self::AccountOverview { account, .. }
            | Self::ComparePeriods { account, .. }
            | Self::CompareCampaigns { account, .. }
            | Self::InvestigateConversions { account }
            | Self::InvestigateSearchTerms { account, .. }
            | Self::InvestigateSegments { account, .. }
            | Self::ReviewEconomics { account }
            | Self::ReviewHistory { account }
            | Self::BusinessContext { account } => Some(*account),
        }
    }
}

/// Strict tool arguments: each operation is a closed object with all fields required.
pub fn schema() -> serde_json::Value {
    use serde_json::{json, Map, Value};
    let choices = |values: &[&str]| json!({"type":"string","enum":values});
    let period = choices(&["last_7_days", "last_30_days"]);
    let account = json!({"type":"string","pattern":"^A[1-9]$"});
    let rows = json!({"type":"integer","minimum":1,"maximum":MAX_ROWS});
    let definitions = vec![
        ("rank_accounts", vec![("period", period.clone())]),
        (
            "account_overview",
            vec![("account", account.clone()), ("period", period.clone())],
        ),
        (
            "compare_periods",
            vec![("account", account.clone()), ("period", period.clone())],
        ),
        (
            "compare_campaigns",
            vec![
                ("account", account.clone()),
                ("period", period),
                (
                    "order",
                    choices(&[
                        "spend",
                        "spend_change",
                        "conversion_change",
                        "cost_per_conversion_change",
                    ]),
                ),
                ("rows", rows.clone()),
            ],
        ),
        (
            "investigate_conversions",
            vec![("account", account.clone())],
        ),
        (
            "investigate_search_terms",
            vec![
                ("account", account.clone()),
                ("campaign", json!({"type":["integer","null"]})),
                (
                    "focus",
                    choices(&["top_spend", "spend_without_conversions"]),
                ),
                ("rows", rows),
            ],
        ),
        (
            "investigate_segments",
            vec![
                ("account", account.clone()),
                (
                    "segment",
                    choices(&["device", "location", "hour", "demographic"]),
                ),
            ],
        ),
        ("review_economics", vec![("account", account.clone())]),
        ("review_history", vec![("account", account.clone())]),
        ("business_context", vec![("account", account)]),
    ];
    let objects = definitions.into_iter().map(|(name, fields)| {
        let mut properties = Map::new();
        properties.insert("operation".into(), choices(&[name]));
        for (key, value) in fields { properties.insert(key.into(), value); }
        let required = properties.keys().cloned().collect::<Vec<_>>();
        json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
    }).collect::<Vec<Value>>();
    json!({"anyOf": objects})
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    #[test]
    fn the_user_gets_both_baselines_without_selecting_one_in_the_arguments() {
        let operation =
            json!({"operation":"compare_periods","account":"A1","period":"last_30_days"});
        assert!(serde_json::from_value::<Operation>(operation.clone()).is_ok());
        let mut obsolete = operation;
        obsolete["baseline"] = json!("last_year");
        assert!(serde_json::from_value::<Operation>(obsolete).is_err());
        let schema = schema();
        let comparison = schema["anyOf"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["properties"]["operation"]["enum"][0] == "compare_periods")
            .unwrap();
        assert_eq!(
            comparison["required"],
            json!(["account", "operation", "period"])
        );
    }

    #[test]
    fn the_user_can_request_every_operation_with_only_its_declared_arguments() {
        let examples = [
            json!({"operation":"rank_accounts","period":"last_7_days"}),
            json!({"operation":"account_overview","account":"A1","period":"last_30_days"}),
            json!({"operation":"compare_periods","account":"A1","period":"last_30_days"}),
            json!({"operation":"compare_campaigns","account":"A1","period":"last_30_days","order":"spend","rows":10}),
            json!({"operation":"investigate_conversions","account":"A1"}),
            json!({"operation":"investigate_search_terms","account":"A1","campaign":null,"focus":"top_spend","rows":20}),
            json!({"operation":"investigate_segments","account":"A1","segment":"device"}),
            json!({"operation":"review_economics","account":"A1"}),
            json!({"operation":"review_history","account":"A1"}),
            json!({"operation":"business_context","account":"A1"}),
        ];
        let schema = schema();
        for example in examples {
            let object = schema["anyOf"]
                .as_array()
                .unwrap()
                .iter()
                .find(|object| object["properties"]["operation"]["enum"][0] == example["operation"])
                .unwrap();
            for key in object["required"].as_array().unwrap() {
                assert!(example.get(key.as_str().unwrap()).is_some());
            }
            for (key, value) in example.as_object().unwrap() {
                let property = &object["properties"][key];
                assert_ne!(property, &Value::Null);
                if let Some(values) = property["enum"].as_array() {
                    assert!(values.contains(value));
                }
            }
            for key in object["required"].as_array().unwrap() {
                let mut missing = example.clone();
                missing
                    .as_object_mut()
                    .unwrap()
                    .remove(key.as_str().unwrap());
                assert!(serde_json::from_value::<Operation>(missing).is_err());
            }
            let parsed: Operation = serde_json::from_value(example.clone()).unwrap();
            assert_eq!(serde_json::to_value(parsed).unwrap(), example);
            let mut unknown = example.clone();
            unknown["sql"] = "anything".into();
            assert!(serde_json::from_value::<Operation>(unknown).is_err());
            if example.get("account").is_some() {
                let mut invalid = example.clone();
                invalid["account"] = "4829153307".into();
                assert!(serde_json::from_value::<Operation>(invalid).is_err());
            }
            if example.get("rows").is_some() {
                for invalid in [0, 50] {
                    let mut too_many = example.clone();
                    too_many["rows"] = invalid.into();
                    assert!(serde_json::from_value::<Operation>(too_many).is_err());
                }
            }
        }
        for handle in ["A0", "A10", "A01", "A+1", "a1"] {
            assert!(serde_json::from_value::<Operation>(
                json!({"operation":"business_context","account":handle})
            )
            .is_err());
        }
    }
}
