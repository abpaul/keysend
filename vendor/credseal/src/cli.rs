//! The `credseal` command line, built with the `cli` feature.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, ExitStatus};

use clap::{Args, Parser, Subcommand};
use serde::de::IgnoredAny;
use tempfile::TempDir;
use zeroize::Zeroizing;

use crate::envelope::MAX_PLAINTEXT_BYTES;
use crate::file::{open_regular_file, parse_toml, read_regular_file};
use crate::key::ENVIRONMENT_VARIABLE;
use crate::{Context, CredentialsFile, Error, MasterKey};

/// Runs the `credseal` command line with the process arguments and returns
/// the exit code. Errors are printed to standard error.
///
/// Call it only as the entry point of its own process. On Unix, `edit`
/// catches SIGINT and SIGQUIT for the rest of the process and never restores
/// their previous handling, so that a `Ctrl-C` or `Ctrl-\` typed in the editor
/// cannot stop credseal and leave the plaintext on disk. The editor gets the
/// default handling. Other signals, such as SIGTERM and SIGKILL, still stop
/// credseal and can leave the plaintext in the temporary directory.
pub fn run() -> ExitCode {
    match execute(Cli::parse().command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("credseal: {error}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Parser)]
#[command(
    name = "credseal",
    version,
    about = "Encrypted, per-environment TOML credentials",
    after_help = "CREDSEAL_MASTER_KEY takes precedence. Otherwise use <directory>/<env>.key; init and edit create an ignored local key for new credentials."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Print a new random master key, and nothing else, to standard output
    Keygen,
    /// Create an encrypted credentials file with a commented template
    Init(Target),
    /// Decrypt into a private temporary file, open an editor, and encrypt the result;
    /// a missing file starts from the commented template
    #[command(visible_alias = "credentials:edit")]
    Edit {
        #[command(flatten)]
        target: Target,
        /// Editor command, split like a shell but never run by one [default: $VISUAL, $EDITOR, vi]
        #[arg(long)]
        editor: Option<String>,
    },
    /// Decrypt and check that the credentials are valid TOML, without printing them
    Validate(Target),
}

#[derive(Args)]
struct Target {
    /// Application name: 1 to 64 characters from a-z, 0-9, _ and - [default: the package name in ./Cargo.toml]
    #[arg(long = "app")]
    application: Option<String>,
    /// Environment name, also the file name: 1 to 64 characters from a-z, 0-9, _ and -
    #[arg(short = 'e', long = "environment", visible_alias = "env")]
    environment: String,
    /// Directory of the <env>.toml.enc files
    #[arg(long, default_value = "config/credentials")]
    directory: PathBuf,
}

impl Target {
    fn file(&self) -> Result<CredentialsFile, CliError> {
        let context = match &self.application {
            Some(application) => Context::new(application, &self.environment)?,
            None => {
                Context::new(&package_name()?, &self.environment).map_err(|error| match error {
                    Error::InvalidName {
                        field: "application",
                    } => CliError::Application(ManifestProblem::InvalidName),
                    error => error.into(),
                })?
            }
        };
        let path = self
            .directory
            .join(format!("{}.toml.enc", context.environment()));
        Ok(CredentialsFile::new(path, context))
    }
}

const MANIFEST: &str = "Cargo.toml";
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;

fn package_name() -> Result<String, CliError> {
    let manifest = match read_regular_file(Path::new(MANIFEST), MAX_MANIFEST_BYTES) {
        Ok(manifest) => manifest,
        Err(Error::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
            return Err(CliError::Application(ManifestProblem::Missing))
        }
        Err(error) => return Err(CliError::Application(ManifestProblem::Unreadable(error))),
    };
    let malformed = || CliError::Application(ManifestProblem::Malformed);
    let text = std::str::from_utf8(&manifest).map_err(|_| malformed())?;
    let table = toml::from_str::<toml::Table>(text).map_err(|_| malformed())?;
    table
        .get("package")
        .and_then(|package| package.get("name"))
        .and_then(toml::Value::as_str)
        .map(str::to_owned)
        .ok_or(CliError::Application(ManifestProblem::NoPackage))
}

#[derive(Debug, thiserror::Error)]
enum ManifestProblem {
    #[error("there is no {MANIFEST}")]
    Missing,
    #[error("{}", unreadable(.0))]
    Unreadable(Error),
    #[error("{MANIFEST} is not valid TOML")]
    Malformed,
    #[error("{MANIFEST} has no [package] name")]
    NoPackage,
    #[error("the package name in {MANIFEST} is not 1 to 64 characters from a-z, 0-9, _ and -")]
    InvalidName,
}

fn unreadable(error: &Error) -> String {
    match error {
        Error::TooLarge { limit, .. } => format!("{MANIFEST} is larger than {limit} bytes"),
        error => error.to_string(),
    }
}

#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error(transparent)]
    Credentials(#[from] Error),
    #[error("cannot take the application name from the current directory: {0}; pass --app <APPLICATION>")]
    Application(ManifestProblem),
    #[error("no master key: set CREDSEAL_MASTER_KEY or restore {}", .0.display())]
    MissingLocalKey(PathBuf),
    #[error("refusing local key {} because Git tracks it; remove it from the index and rotate any exposed key", .0.display())]
    TrackedKey(PathBuf),
    #[error("cannot verify Git protection for {}", .0.display())]
    GitProtection(PathBuf),
    #[error("cannot write to standard output: {0}")]
    Output(io::Error),
    #[error("{} is locked by another credseal process", .0.display())]
    Locked(PathBuf),
    #[error("{} already exists; change it with `credseal edit`", .0.display())]
    AlreadyExists(PathBuf),
    #[error("the editor command is empty or has unbalanced quotes")]
    EditorCommand,
    #[error("cannot start the editor {program}: {source}")]
    EditorStart { program: String, source: io::Error },
    #[error("the editor failed with {0}; the credentials file is unchanged and the edits were discarded")]
    EditorFailed(ExitStatus),
    #[error("edited credentials rejected: {0}; the credentials file is unchanged and the edits were discarded")]
    InvalidEdit(Error),
    #[error("{} changed during the edit; the change was kept and the edits were discarded", .0.display())]
    ChangedExternally(PathBuf),
    #[error("cannot catch interrupt signals: {0}")]
    Signals(io::Error),
    #[error("the file was saved to {}, but syncing {} failed: {source}; the save may not survive a crash or power loss", .path.display(), .directory.display())]
    NotSynced {
        path: PathBuf,
        directory: PathBuf,
        source: io::Error,
    },
}

fn execute(command: Commands) -> Result<(), CliError> {
    match command {
        Commands::Keygen => keygen(),
        Commands::Init(target) => init(&target),
        Commands::Edit { target, editor } => edit(&target, editor.as_deref()),
        Commands::Validate(target) => validate(&target),
    }
}

fn keygen() -> Result<(), CliError> {
    let key = MasterKey::generate()?;
    let mut stdout = io::stdout().lock();
    stdout
        .write_all(key.to_base64().as_bytes())
        .and_then(|()| stdout.write_all(b"\n"))
        .and_then(|()| stdout.flush())
        .map_err(CliError::Output)
}

fn init(target: &Target) -> Result<(), CliError> {
    let file = target.file()?;
    let directory = parent(file.path());
    fs::create_dir_all(directory).map_err(|source| Error::io("create", directory, source))?;
    let _lock = lock(file.path())?;
    match fs::symlink_metadata(file.path()) {
        Ok(_) => return Err(CliError::AlreadyExists(file.path().to_path_buf())),
        Err(source) if source.kind() == io::ErrorKind::NotFound => {}
        Err(source) => return Err(Error::io("inspect", file.path(), source).into()),
    }
    let key = resolve_key(target, true)?;
    let sealed = file.encrypt(&key, template(file.context()).as_bytes())?;
    write_atomically(file.path(), &sealed, Destination::New)?;
    eprintln!("Created {}", file.path().display());
    Ok(())
}

fn template(context: &Context) -> String {
    format!(
        "# Credentials for {} in {}, as TOML.\n\
         # Example:\n\
         #\n\
         # [stripe]\n\
         # secret_key = \"sk_live_...\"\n",
        context.application(),
        context.environment(),
    )
}

fn edit(target: &Target, editor: Option<&str>) -> Result<(), CliError> {
    let file = target.file()?;
    let editor = Editor::choose(editor)?;
    let directory = parent(file.path());
    fs::create_dir_all(directory).map_err(|source| Error::io("create", directory, source))?;
    let _lock = lock(file.path())?;
    let original = match file.read() {
        Ok(original) => Some(original),
        Err(Error::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let key = resolve_key(target, original.is_none())?;
    let plaintext = match &original {
        Some(original) => file.decrypt(&key, original)?,
        None => Zeroizing::new(template(file.context()).into_bytes()),
    };

    keep_running_on_terminal_signals()?;
    let (workspace, plaintext_path) = write_workspace(file.context(), &plaintext)?;
    editor.open(&plaintext_path)?;
    let edited =
        read_regular_file(&plaintext_path, MAX_PLAINTEXT_BYTES).map_err(CliError::InvalidEdit)?;
    drop(workspace);

    let Some(original) = original else {
        parse_toml(&edited).map_err(CliError::InvalidEdit)?;
        let sealed = file.encrypt(&key, &edited)?;
        write_atomically(file.path(), &sealed, Destination::New).map_err(|error| match error {
            CliError::AlreadyExists(path) => CliError::ChangedExternally(path),
            error => error,
        })?;
        eprintln!("Created {}", file.path().display());
        return Ok(());
    };
    if edited[..] == plaintext[..] {
        eprintln!("No changes; {} is unchanged", file.path().display());
        return Ok(());
    }
    parse_toml(&edited).map_err(CliError::InvalidEdit)?;
    let sealed = file.encrypt(&key, &edited)?;
    match file.read() {
        Ok(current) if current[..] == original[..] => {}
        Ok(_) => return Err(CliError::ChangedExternally(file.path().to_path_buf())),
        Err(Error::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
            return Err(CliError::ChangedExternally(file.path().to_path_buf()))
        }
        Err(error) => return Err(error.into()),
    }
    write_atomically(file.path(), &sealed, Destination::Existing)?;
    eprintln!("Saved {}", file.path().display());
    Ok(())
}

fn validate(target: &Target) -> Result<(), CliError> {
    let file = target.file()?;
    let key = resolve_key(target, false)?;
    file.load::<IgnoredAny>(&key)?;
    println!("{} is valid", file.path().display());
    Ok(())
}

fn resolve_key(target: &Target, create: bool) -> Result<MasterKey, CliError> {
    if std::env::var_os(ENVIRONMENT_VARIABLE).is_some() {
        return Ok(MasterKey::from_env()?);
    }
    let path = target.directory.join(format!("{}.key", target.environment));
    let git = git_worktree(&target.directory)?;
    if git
        && git_status(
            &target.directory,
            &["ls-files", "--error-unmatch", "--"],
            &path,
        )?
    {
        return Err(CliError::TrackedKey(path));
    }
    match read_regular_file(&path, 128) {
        Ok(encoded) => {
            let text = std::str::from_utf8(&encoded).map_err(|_| Error::MalformedKey)?;
            return Ok(MasterKey::from_base64(text)?);
        }
        Err(Error::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    if !create {
        return Err(CliError::MissingLocalKey(path));
    }
    protect_local_key(target, &path, git)?;
    let key = MasterKey::generate()?;
    let encoded = key.to_base64();
    write_atomically(&path, encoded.as_bytes(), Destination::New)?;
    eprintln!(
        "Created local master key {}; keep it private and backed up",
        path.display()
    );
    Ok(key)
}

fn git_worktree(directory: &Path) -> Result<bool, CliError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output();
    match output {
        Err(error) if error.kind() == io::ErrorKind::NotFound && !has_git_metadata(directory)? => {
            Ok(false)
        }
        Ok(output) if output.status.success() && output.stdout == b"true\n" => Ok(true),
        Ok(output) if output.status.code() == Some(128) && !has_git_metadata(directory)? => {
            Ok(false)
        }
        _ => Err(CliError::GitProtection(directory.to_path_buf())),
    }
}

fn has_git_metadata(directory: &Path) -> Result<bool, CliError> {
    let mut absolute = if directory.is_absolute() {
        directory.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|source| Error::io("inspect", directory, source))?
            .join(directory)
    };
    loop {
        match fs::canonicalize(&absolute) {
            Ok(existing) => {
                absolute = existing;
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound && absolute.pop() => {}
            Err(_) => return Err(CliError::GitProtection(directory.to_path_buf())),
        }
    }
    for ancestor in absolute.ancestors() {
        match fs::symlink_metadata(ancestor.join(".git")) {
            Ok(_) => return Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => return Err(CliError::GitProtection(directory.to_path_buf())),
        }
    }
    Ok(false)
}

fn git_status(directory: &Path, arguments: &[&str], path: &Path) -> Result<bool, CliError> {
    let absolute = fs::canonicalize(directory)
        .map_err(|source| Error::io("inspect", directory, source))?
        .join(path.file_name().expect("key file has a name"));
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(arguments)
        .arg(absolute)
        .output()
        .map_err(|_| CliError::GitProtection(path.to_path_buf()))?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(CliError::GitProtection(path.to_path_buf())),
    }
}

fn protect_local_key(target: &Target, key_path: &Path, git: bool) -> Result<(), CliError> {
    let path = target.directory.join(".gitignore");
    let mut options = OpenOptions::new();
    options.read(true).append(true).create(true);
    let mut ignore = open_regular_file(&path, &mut options)?;
    ignore
        .lock()
        .map_err(|source| Error::io("lock", &path, source))?;
    let mut contents = String::new();
    (&mut ignore)
        .take(64 * 1024 + 1)
        .read_to_string(&mut contents)
        .map_err(|source| Error::io("read", &path, source))?;
    if contents.len() > 64 * 1024 {
        return Err(Error::TooLarge {
            path,
            limit: 64 * 1024,
        }
        .into());
    }
    let rule = format!("/{}.key", target.environment);
    if contents.lines().next_back() != Some(rule.as_str()) {
        if !contents.is_empty() && !contents.ends_with('\n') {
            ignore
                .write_all(b"\n")
                .map_err(|source| Error::io("write", &path, source))?;
        }
        writeln!(ignore, "{rule}").map_err(|source| Error::io("write", &path, source))?;
    }
    ignore
        .sync_all()
        .map_err(|source| Error::io("sync", &path, source))?;
    sync_directory(&target.directory)
        .map_err(|source| Error::io("sync", &target.directory, source))?;
    if git && !git_status(&target.directory, &["check-ignore", "-q", "--"], key_path)? {
        return Err(CliError::GitProtection(key_path.to_path_buf()));
    }
    Ok(())
}

#[cfg(unix)]
fn keep_running_on_terminal_signals() -> Result<(), CliError> {
    use signal_hook::consts::{SIGINT, SIGQUIT};
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    let caught = Arc::new(AtomicBool::new(false));
    for signal in [SIGINT, SIGQUIT] {
        signal_hook::flag::register(signal, Arc::clone(&caught)).map_err(CliError::Signals)?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn keep_running_on_terminal_signals() -> Result<(), CliError> {
    Ok(())
}

struct Editor {
    program: String,
    arguments: Vec<String>,
}

impl Editor {
    fn choose(explicit: Option<&str>) -> Result<Self, CliError> {
        let command = match explicit {
            Some(command) => command.to_owned(),
            None => ["VISUAL", "EDITOR"]
                .into_iter()
                .filter_map(|name| std::env::var(name).ok())
                .find(|command| !command.trim().is_empty())
                .unwrap_or_else(|| "vi".to_owned()),
        };
        let mut words = shell_words::split(&command)
            .map_err(|_| CliError::EditorCommand)?
            .into_iter();
        let program = words.next().ok_or(CliError::EditorCommand)?;
        Ok(Self {
            program,
            arguments: words.collect(),
        })
    }

    fn open(&self, path: &Path) -> Result<(), CliError> {
        let status = Command::new(&self.program)
            .args(&self.arguments)
            .arg(path)
            .env_remove(ENVIRONMENT_VARIABLE)
            .status()
            .map_err(|source| CliError::EditorStart {
                program: self.program.clone(),
                source,
            })?;
        if status.success() {
            Ok(())
        } else {
            Err(CliError::EditorFailed(status))
        }
    }
}

fn write_workspace(context: &Context, plaintext: &[u8]) -> Result<(TempDir, PathBuf), Error> {
    let mut builder = tempfile::Builder::new();
    builder.prefix("credseal-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(fs::Permissions::from_mode(0o700));
    }
    let workspace = builder
        .tempdir()
        .map_err(|source| Error::io("create", &std::env::temp_dir(), source))?;
    let path = workspace
        .path()
        .join(format!("{}.toml", context.environment()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(&path)
        .and_then(|mut file| file.write_all(plaintext))
        .map_err(|source| Error::io("write", &path, source))?;
    Ok((workspace, path))
}

fn lock(path: &Path) -> Result<File, CliError> {
    let mut lock_path = path.as_os_str().to_owned();
    lock_path.push(".lock");
    let lock_path = PathBuf::from(lock_path);
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = open_regular_file(&lock_path, &mut options)?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => Err(CliError::Locked(path.to_path_buf())),
        Err(TryLockError::Error(source)) => Err(Error::io("lock", &lock_path, source).into()),
    }
}

enum Destination {
    New,
    Existing,
}

fn write_atomically(
    path: &Path,
    contents: &[u8],
    destination: Destination,
) -> Result<(), CliError> {
    let directory = parent(path);
    let mut temporary = tempfile::Builder::new()
        .prefix(".credseal-")
        .tempfile_in(directory)
        .map_err(|source| Error::io("create a file in", directory, source))?;
    temporary
        .write_all(contents)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|source| Error::io("write", temporary.path(), source))?;
    match destination {
        Destination::New => temporary.persist_noclobber(path).map_err(|error| {
            if error.error.kind() == io::ErrorKind::AlreadyExists {
                CliError::AlreadyExists(path.to_path_buf())
            } else {
                Error::io("create", path, error.error).into()
            }
        })?,
        Destination::Existing => temporary
            .persist(path)
            .map_err(|error| Error::io("replace", path, error.error))?,
    };
    sync_directory(directory).map_err(|source| CliError::NotSynced {
        path: path.to_path_buf(),
        directory: directory.to_path_buf(),
        source,
    })
}

#[cfg(unix)]
fn sync_directory(directory: &Path) -> io::Result<()> {
    File::open(directory).and_then(|handle| handle.sync_all())
}

#[cfg(not(unix))]
fn sync_directory(_directory: &Path) -> io::Result<()> {
    Ok(())
}

fn parent(path: &Path) -> &Path {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}
