use std::fs::{File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::de::{DeserializeOwned, IgnoredAny};
use zeroize::Zeroizing;

use crate::envelope::{self, MAX_FILE_BYTES};
use crate::{Context, Error, MasterKey, Result};

/// An encrypted credentials file bound to a [`Context`].
///
/// Loading reads the file into memory and decrypts it there. It never writes
/// decrypted text to disk.
///
/// ```no_run
/// use credseal::{Context, CredentialsFile, MasterKey, SecretString};
///
/// #[derive(serde::Deserialize)]
/// struct Credentials {
///     database_url: SecretString,
/// }
///
/// let key = MasterKey::from_env()?;
/// let context = Context::new("myapp", "production")?;
/// let file = CredentialsFile::new("config/credentials/production.toml.enc", context);
/// let credentials: Credentials = file.load(&key)?;
/// let _url: &str = credentials.database_url.expose_secret();
/// # Ok::<(), credseal::Error>(())
/// ```
#[derive(Debug, Clone)]
pub struct CredentialsFile {
    path: PathBuf,
    context: Context,
}

impl CredentialsFile {
    /// Names the file and the application and environment it must belong to.
    /// Nothing is read until [`CredentialsFile::load`].
    pub fn new(path: impl Into<PathBuf>, context: Context) -> Self {
        Self {
            path: path.into(),
            context,
        }
    }

    /// The path of the encrypted file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The application and environment the file must belong to.
    pub fn context(&self) -> &Context {
        &self.context
    }

    /// Reads, decrypts and deserializes the credentials.
    ///
    /// The file must be a regular file, not a symbolic link, of at most
    /// 2 MiB, and the decrypted TOML at most 1 MiB. Deserialization failures
    /// report only the line, never the message of the failing
    /// `Deserialize` implementation.
    pub fn load<T: DeserializeOwned>(&self, key: &MasterKey) -> Result<T> {
        let file = self.read()?;
        let plaintext = self.decrypt(key, &file)?;
        let text = parse_toml(&plaintext)?;
        toml::from_str(text).map_err(|error| Error::Deserialize {
            line: line_of(text, &error),
        })
    }

    pub(crate) fn read(&self) -> Result<Zeroizing<Vec<u8>>> {
        read_regular_file(&self.path, MAX_FILE_BYTES)
    }

    pub(crate) fn decrypt(&self, key: &MasterKey, file: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        envelope::open(key, &self.context, file)
    }

    #[cfg(feature = "cli")]
    pub(crate) fn encrypt(&self, key: &MasterKey, plaintext: &[u8]) -> Result<Vec<u8>> {
        if plaintext.len() > envelope::MAX_PLAINTEXT_BYTES {
            return Err(Error::TooLarge {
                path: self.path.clone(),
                limit: envelope::MAX_PLAINTEXT_BYTES,
            });
        }
        envelope::seal(key, &self.context, plaintext)
    }
}

pub(crate) fn parse_toml(plaintext: &[u8]) -> Result<&str> {
    let text = std::str::from_utf8(plaintext).map_err(|_| Error::InvalidToml { line: None })?;
    toml::from_str::<IgnoredAny>(text).map_err(|error| Error::InvalidToml {
        line: line_of(text, &error),
    })?;
    Ok(text)
}

fn line_of(text: &str, error: &toml::de::Error) -> Option<usize> {
    let start = error.span()?.start;
    let before = text.as_bytes().get(..start)?;
    Some(before.iter().filter(|byte| **byte == b'\n').count() + 1)
}

pub(crate) fn read_regular_file(path: &Path, limit: usize) -> Result<Zeroizing<Vec<u8>>> {
    let file = open_regular_file(path, OpenOptions::new().read(true))?;
    let length = file
        .metadata()
        .map_err(|source| Error::io("read", path, source))?
        .len();
    let capacity = usize::try_from(length).unwrap_or(usize::MAX).min(limit) + 1;
    let mut contents = Zeroizing::new(Vec::with_capacity(capacity));
    file.take(limit as u64 + 1)
        .read_to_end(&mut contents)
        .map_err(|source| Error::io("read", path, source))?;
    if contents.len() > limit {
        return Err(Error::TooLarge {
            path: path.to_path_buf(),
            limit,
        });
    }
    Ok(contents)
}

pub(crate) fn open_regular_file(path: &Path, options: &mut OpenOptions) -> Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(not(unix))]
    if std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_symlink()) {
        return Err(Error::NotRegularFile {
            path: path.to_path_buf(),
        });
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(source) if is_symlink_refusal(&source) => {
            return Err(Error::NotRegularFile {
                path: path.to_path_buf(),
            })
        }
        Err(source) => return Err(Error::io("open", path, source)),
    };
    let metadata = file
        .metadata()
        .map_err(|source| Error::io("open", path, source))?;
    if !metadata.is_file() {
        return Err(Error::NotRegularFile {
            path: path.to_path_buf(),
        });
    }
    Ok(file)
}

fn is_symlink_refusal(error: &std::io::Error) -> bool {
    #[cfg(unix)]
    {
        error.raw_os_error() == Some(libc::ELOOP)
    }
    #[cfg(not(unix))]
    {
        let _ = error;
        false
    }
}
