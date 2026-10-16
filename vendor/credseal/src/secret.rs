use std::fmt;

use serde::{Deserialize, Deserializer};
use zeroize::Zeroizing;

/// A secret string field in a credentials type.
///
/// The string is zeroized on drop. `Debug` prints `SecretString([REDACTED])`.
/// It has no `Display` or `Serialize`, so it cannot reach logs or output by
/// accident. Call [`SecretString::expose_secret`] where the value is used.
///
/// The TOML parser and `serde` make their own temporary copies while they
/// decode; credseal cannot zeroize those.
pub struct SecretString(Zeroizing<String>);

impl SecretString {
    /// Returns the secret value.
    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretString([REDACTED])")
    }
}

impl<'de> Deserialize<'de> for SecretString {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(|value| Self(Zeroizing::new(value)))
    }
}
