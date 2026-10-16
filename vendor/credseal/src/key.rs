use std::fmt;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use zeroize::Zeroizing;

use crate::{Error, Result};

pub(crate) const KEY_BYTES: usize = 32;
const ENCODED_KEY_BYTES: usize = 44;
pub(crate) const ENVIRONMENT_VARIABLE: &str = "CREDSEAL_MASTER_KEY";

/// A 256-bit AES-GCM master key.
///
/// The key bytes are zeroized on drop. `Debug` prints `MasterKey([REDACTED])`.
/// The library reads keys from `CREDSEAL_MASTER_KEY` or from a string the
/// application supplies. The optional CLI also manages ignored local key files.
pub struct MasterKey(Zeroizing<[u8; KEY_BYTES]>);

impl MasterKey {
    /// Reads the key from the `CREDSEAL_MASTER_KEY` environment variable.
    ///
    /// Returns [`Error::MissingKey`] when the variable is unset and
    /// [`Error::MalformedKey`] when it is not valid Unicode or not a key.
    pub fn from_env() -> Result<Self> {
        let value = std::env::var_os(ENVIRONMENT_VARIABLE).ok_or(Error::MissingKey)?;
        let value = Zeroizing::new(value.into_string().map_err(|_| Error::MalformedKey)?);
        Self::from_base64(&value)
    }

    /// Decodes standard padded Base64 of exactly 32 bytes. Surrounding ASCII
    /// whitespace is ignored.
    pub fn from_base64(encoded: &str) -> Result<Self> {
        let encoded = encoded.trim_ascii();
        if encoded.len() != ENCODED_KEY_BYTES {
            return Err(Error::MalformedKey);
        }
        let mut decoded = Zeroizing::new([0u8; ENCODED_KEY_BYTES]);
        let length = STANDARD
            .decode_slice(encoded, &mut decoded[..])
            .map_err(|_| Error::MalformedKey)?;
        if length != KEY_BYTES {
            return Err(Error::MalformedKey);
        }
        let mut key = Zeroizing::new([0u8; KEY_BYTES]);
        key.copy_from_slice(&decoded[..KEY_BYTES]);
        Ok(Self(key))
    }

    /// Creates a key from the operating system random number generator.
    pub fn generate() -> Result<Self> {
        let mut key = Zeroizing::new([0u8; KEY_BYTES]);
        getrandom::fill(&mut key[..]).map_err(|_| Error::Random)?;
        Ok(Self(key))
    }

    /// Encodes the key as standard padded Base64, the form that
    /// [`MasterKey::from_base64`] and `CREDSEAL_MASTER_KEY` accept.
    ///
    /// This is the only way to see the key. Store the result in a secret
    /// manager, never in the repository.
    pub fn to_base64(&self) -> Zeroizing<String> {
        let mut encoded = Zeroizing::new(String::with_capacity(ENCODED_KEY_BYTES));
        STANDARD.encode_string(&self.0[..], &mut encoded);
        encoded
    }

    pub(crate) fn bytes(&self) -> &[u8; KEY_BYTES] {
        &self.0
    }
}

impl fmt::Debug for MasterKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MasterKey([REDACTED])")
    }
}
