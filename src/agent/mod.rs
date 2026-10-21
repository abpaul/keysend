//! The Agent: a read-only investigation of the Ad Accounts of one Workspace, asked in words.
//!
//! The model chooses operations from [`operation::Operation`] and words the answer. Code
//! resolves the Ad Accounts from the session, reads the store, computes every number, gives
//! each piece of evidence its strength, and keeps only the claims that their cited evidence
//! supports. No operation writes to Google Ads or to the store of another Workspace.

#[cfg(test)]
pub(crate) mod eval;

pub mod answer;
pub mod data;
pub mod evidence;
pub mod investigation;
pub mod operation;
pub mod run;
pub mod worker;

use serde::{Deserialize, Serialize};

/// An Ad Account as the model sees it: a short handle that code resolves within the
/// investigation. The model never sees or sends a Google Ads customer ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Handle(u8);

impl Handle {
    pub fn new(index: usize) -> Self {
        Self(u8::try_from(index + 1).unwrap_or(u8::MAX))
    }

    pub fn index(self) -> usize {
        usize::from(self.0) - 1
    }
}

impl TryFrom<String> for Handle {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value
            .strip_prefix('A')
            .and_then(|number| number.parse::<u8>().ok())
            .filter(|number| *number > 0)
            .map(Self)
            .ok_or("unknown Ad Account")
    }
}

impl From<Handle> for String {
    fn from(handle: Handle) -> Self {
        format!("A{}", handle.0)
    }
}

/// The lifecycle of an investigation. Only these transitions exist:
/// Created → Gathering → Answering → Completed, and any state before Completed → Failed or
/// Cancelled. `Answering` covers the final model call and the check of its claims.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Created,
    Gathering,
    Answering,
    Completed,
    Failed,
    Cancelled,
}

impl State {
    pub fn finished(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    pub fn may_become(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Created, Self::Gathering)
                | (Self::Gathering, Self::Answering)
                | (Self::Answering, Self::Completed)
        ) || (!self.finished() && matches!(next, Self::Failed | Self::Cancelled))
    }
}

/// Why an investigation stopped without an answer. Each is shown to the User with what to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Failure {
    /// The Workspace reached its daily number of investigations.
    DailyLimit,
    /// The investigation reached its time or token budget before an answer.
    Budget,
    /// The AI service did not answer, or answered outside the schema twice.
    Model,
    /// The Workspace has no Access, or no Ad Account in the investigation.
    NoAccess,
    /// The job stopped while running and was not completed.
    Interrupted,
}
