use crate::{Error, Result};

const MAX_NAME_BYTES: usize = 64;

/// The application and environment that credentials belong to.
///
/// Both names are authenticated as associated data. A file encrypted for one
/// application or environment fails to decrypt for another, even with the
/// same key. The environment is also the file stem used by the CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Context {
    application: String,
    environment: String,
}

impl Context {
    /// Validates both names: 1 to 64 bytes from `a-z`, `0-9`, `_` and `-`.
    ///
    /// ```
    /// let context = credseal::Context::new("myapp", "production")?;
    /// assert_eq!(context.environment(), "production");
    /// assert!(credseal::Context::new("myapp", "../prod").is_err());
    /// # Ok::<(), credseal::Error>(())
    /// ```
    pub fn new(application: &str, environment: &str) -> Result<Self> {
        Ok(Self {
            application: name("application", application)?,
            environment: name("environment", environment)?,
        })
    }

    /// The application name.
    pub fn application(&self) -> &str {
        &self.application
    }

    /// The environment name.
    pub fn environment(&self) -> &str {
        &self.environment
    }
}

fn name(field: &'static str, value: &str) -> Result<String> {
    let valid = !value.is_empty()
        && value.len() <= MAX_NAME_BYTES
        && value
            .bytes()
            .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'));
    if valid {
        Ok(value.to_owned())
    } else {
        Err(Error::InvalidName { field })
    }
}
