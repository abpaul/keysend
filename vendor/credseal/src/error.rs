use std::io;
use std::path::{Path, PathBuf};

/// A failure to read, decrypt or decode credentials.
///
/// `Display`, `Debug` and [`std::error::Error::source`] never contain the
/// master key, decrypted text, TOML excerpts or messages from `Deserialize`
/// implementations. They may contain file paths and operating system error
/// descriptions.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// An application or environment name is not 1 to 64 characters from
    /// `a-z`, `0-9`, `_` and `-`.
    #[error("invalid {field} name: use 1 to 64 characters from a-z, 0-9, _ and -")]
    InvalidName {
        /// `"application"` or `"environment"`.
        field: &'static str,
    },
    /// `CREDSEAL_MASTER_KEY` is not set.
    #[error("CREDSEAL_MASTER_KEY is not set")]
    MissingKey,
    /// The master key is not standard padded Base64 of exactly 32 bytes.
    #[error("the master key is not Base64 of exactly 32 bytes")]
    MalformedKey,
    /// The operating system random number generator failed.
    #[error("the operating system random number generator failed")]
    Random,
    /// A file operation failed.
    #[error("cannot {operation} {}: {source}", path.display())]
    Io {
        /// What credseal tried to do, such as `"read"`.
        operation: &'static str,
        /// The file or directory.
        path: PathBuf,
        /// The operating system error.
        #[source]
        source: io::Error,
    },
    /// The path is a symbolic link, a directory or another non-regular file.
    #[error("{} is not a regular file", path.display())]
    NotRegularFile {
        /// The rejected path.
        path: PathBuf,
    },
    /// The file is larger than credseal accepts.
    #[error("{} is larger than the limit of {limit} bytes", path.display())]
    TooLarge {
        /// The rejected file.
        path: PathBuf,
        /// The limit in bytes.
        limit: usize,
    },
    /// The file starts with a credseal header of a version this release
    /// cannot read.
    #[error("unsupported credseal file version")]
    UnsupportedVersion,
    /// The file is not a credseal v1 file, or its Base64 or length is wrong.
    #[error("malformed credseal file")]
    Malformed,
    /// Authentication failed: the key is wrong, the application or
    /// environment differs from the authenticated context, or the file
    /// content does not match its authentication tag.
    #[error(
        "cannot decrypt credentials: wrong key, wrong application or environment, or modified file"
    )]
    Decrypt,
    /// The decrypted text is not UTF-8 TOML.
    #[error("credentials are not valid TOML{}", at_line(*line))]
    InvalidToml {
        /// The line of the error, when known.
        line: Option<usize>,
    },
    /// The TOML is valid but does not deserialize into the requested type.
    #[error("credentials do not match the expected type{}", at_line(*line))]
    Deserialize {
        /// The line of the error, when known.
        line: Option<usize>,
    },
}

/// The result type of this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    pub(crate) fn io(operation: &'static str, path: &Path, source: io::Error) -> Self {
        Self::Io {
            operation,
            path: path.to_path_buf(),
            source,
        }
    }
}

fn at_line(line: Option<usize>) -> String {
    line.map(|line| format!(" (line {line})"))
        .unwrap_or_default()
}
