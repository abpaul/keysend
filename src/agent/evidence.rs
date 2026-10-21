//! Evidence: what an operation returns. Each piece has an ID that a claim cites, the Ad
//! Account and days it covers, the numbers as KeySend displays them, how fresh and complete
//! it is, and a strength that code sets from counts. The model never sets a strength.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::Handle;

/// `E<step>.<n>`: the n-th piece of evidence of the step-th operation of an investigation.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EvidenceId(pub String);

impl EvidenceId {
    pub fn new(step: usize, item: usize) -> Self {
        Self(format!("E{step}.{item}"))
    }
}

/// Where the numbers come from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    AccountHistory,
    ConversionTracking,
    /// A Deep check, with the day it was collected.
    DeepCheck {
        day: i64,
    },
    Finding {
        key: String,
    },
    Assessment {
        day: i64,
    },
    RecommendationHistory,
    AccountChanges,
    AdAccountProfile,
    BusinessContext {
        researched_day: Option<i64>,
    },
}

/// What a piece of evidence is about.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Subject {
    AdAccount,
    Campaign { id: i64, name: String },
    SearchTerm { text: String },
    Segment { name: String },
    ConversionAction { name: String },
    Recommendation { finding_key: String },
}

impl Subject {
    /// The name that a claim or a Recommendation uses for the subject.
    pub fn name(&self) -> Option<&str> {
        match self {
            Self::AdAccount => None,
            Self::Campaign { name, .. }
            | Self::SearchTerm { text: name }
            | Self::Segment { name }
            | Self::ConversionAction { name }
            | Self::Recommendation { finding_key: name } => Some(name),
        }
    }
}

/// Inclusive days, as day numbers of the Account history.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Days {
    pub start: i64,
    pub end: i64,
}

/// How much a reader can rest on a piece of evidence. Code sets it from the number of
/// conversions, the days of data and Conversion tracking.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Strength {
    Weak,
    Limited,
    Strong,
}

/// A limit of a piece of evidence that the answer must show.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Caveat {
    /// The last days still miss conversions that Google reports later.
    ConversionsNotSettled { days: i64 },
    /// Too few conversions in a period to judge a change in them.
    FewConversions { conversions: i64 },
    /// Conversion tracking is not working, so conversions cannot be judged.
    TrackingNotWorking,
    /// The Deep check keeps only the search terms with the most spend.
    SearchTermsIncomplete {
        shown: usize,
        of_spend_share: Option<u8>,
    },
    /// The data is the latest Deep check, not the requested period.
    Snapshot { day: i64 },
    /// More rows existed than were returned.
    RowsLeftOut { rows: usize },
    /// The Account history does not reach back to the baseline.
    NoBaseline,
    /// A value that the Ad Account profile does not hold.
    MissingInput { input: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    pub id: EvidenceId,
    pub account: Handle,
    pub source: Source,
    pub subject: Subject,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub days: Option<Days>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline: Option<Days>,
    /// The numbers and labels as KeySend displays them, such as `{"Spend": "$1,204",
    /// "Cost per conversion": "up 12%"}`. Claims are checked against these strings.
    pub facts: Value,
    pub strength: Strength,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub caveats: Vec<Caveat>,
}
