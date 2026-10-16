//! Encrypted, per-environment TOML credentials with a Rails-style workflow.
//!
//! Each environment has one file, such as
//! `config/credentials/production.toml.enc`, encrypted with AES-256-GCM
//! under its own 32-byte master key. The key comes from
//! `CREDSEAL_MASTER_KEY`. The application and environment names are
//! authenticated, so a file only decrypts for the [`Context`] it was written
//! for.
//!
//! ```no_run
//! use credseal::{Context, CredentialsFile, MasterKey, SecretString};
//!
//! #[derive(serde::Deserialize)]
//! struct Credentials {
//!     stripe: Stripe,
//! }
//!
//! #[derive(serde::Deserialize)]
//! struct Stripe {
//!     secret_key: SecretString,
//! }
//!
//! let key = MasterKey::from_env()?;
//! let context = Context::new("myapp", "production")?;
//! let credentials: Credentials =
//!     CredentialsFile::new("config/credentials/production.toml.enc", context).load(&key)?;
//! let _secret: &str = credentials.stripe.secret_key.expose_secret();
//! # Ok::<(), credseal::Error>(())
//! ```
//!
//! # Limits
//!
//! An encrypted file is at most 2 MiB and its decrypted TOML at most 1 MiB.
//!
//! # Memory
//!
//! credseal zeroizes the key, the decrypted text and [`SecretString`] values
//! it owns. The TOML parser and `serde` make temporary copies that are not
//! zeroized, and the operating system may swap memory to disk.
//!
//! # Command line
//!
//! The `cli` feature builds the `credseal` binary with the `keygen`, `init`,
//! `edit` and `validate` commands. See the README.

mod context;
mod envelope;
mod error;
mod file;
mod key;
mod secret;

#[cfg(feature = "cli")]
pub mod cli;

pub use context::Context;
pub use error::{Error, Result};
pub use file::CredentialsFile;
pub use key::MasterKey;
pub use secret::SecretString;
