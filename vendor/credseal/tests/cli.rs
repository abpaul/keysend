//! The `credseal` binary, run as a subprocess.

#![cfg(unix)]

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use credseal::{Context, CredentialsFile, MasterKey, SecretString};
use serde::Deserialize;
use support::{KEY_BASE64, OTHER_KEY_BASE64};
use tempfile::TempDir;

const DEVELOPMENT: &str = "config/credentials/development.toml.enc";
const UNCHANGED: &str = "the credentials file is unchanged and the edits were discarded";

struct Project {
    root: TempDir,
}

impl Project {
    fn new() -> Self {
        let project = Self {
            root: TempDir::new().unwrap(),
        };
        fs::create_dir(project.temporary()).unwrap();
        project
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.path().join(relative)
    }

    fn temporary(&self) -> PathBuf {
        self.path("tmp")
    }

    fn command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_credseal"));
        command
            .args(arguments)
            .current_dir(self.root.path())
            .env("CREDSEAL_MASTER_KEY", KEY_BASE64)
            .env("TMPDIR", self.temporary())
            .env_remove("VISUAL")
            .env_remove("EDITOR")
            .stdin(Stdio::null());
        command
    }

    fn run(&self, arguments: &[&str]) -> Output {
        self.command(arguments).output().unwrap()
    }

    fn init(&self, environment: &str) {
        let output = self.run(&["init", "--app", "myapp", "--env", environment]);
        assert_eq!(
            stderr(&output),
            format!("Created config/credentials/{environment}.toml.enc\n")
        );
    }

    fn editor(&self, name: &str, body: &str) -> PathBuf {
        let directory = self.path("my editors");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(format!("{name}.sh"));
        fs::write(&path, format!("#!/bin/sh\nfor file; do :; done\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn edit(&self, environment: &str, editor: &Path) -> Output {
        self.run(&[
            "edit",
            "--app",
            "myapp",
            "--env",
            environment,
            "--editor",
            &quoted(editor),
        ])
    }

    fn manifest(&self, contents: &str) {
        fs::write(self.path("Cargo.toml"), contents).unwrap();
    }

    fn bytes(&self, relative: &str) -> Vec<u8> {
        fs::read(self.path(relative)).unwrap()
    }

    fn load<T: serde::de::DeserializeOwned>(&self, environment: &str) -> T {
        CredentialsFile::new(
            self.path(&format!("config/credentials/{environment}.toml.enc")),
            Context::new("myapp", environment).unwrap(),
        )
        .load(&MasterKey::from_base64(KEY_BASE64).unwrap())
        .unwrap()
    }

    fn assert_no_plaintext_left(&self) {
        let left: Vec<_> = fs::read_dir(self.temporary()).unwrap().collect();
        assert_eq!(left.len(), 0, "temporary files left: {left:?}");
    }
}

fn quoted(path: &Path) -> String {
    format!("'{}'", path.display())
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

struct HeldEdit {
    child: Option<Child>,
    release: PathBuf,
}

impl HeldEdit {
    fn finish(mut self) -> Output {
        fs::write(&self.release, "").unwrap();
        self.child.take().unwrap().wait_with_output().unwrap()
    }
}

impl Drop for HeldEdit {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        let _ = fs::write(&self.release, "");
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}

#[derive(Deserialize)]
struct Credentials {
    stripe: Stripe,
}

#[derive(Deserialize)]
struct Stripe {
    secret_key: SecretString,
}

#[test]
fn keygen_prints_only_a_base64_key_of_32_random_bytes() {
    let project = Project::new();

    let first = project.run(&["keygen"]);
    let second = project.run(&["keygen"]);

    assert!(first.status.success());
    assert_eq!(stderr(&first), "");
    let key = stdout(&first);
    let encoded = key.strip_suffix('\n').unwrap();
    assert_eq!(encoded.len(), 44);
    assert_eq!(STANDARD.decode(encoded).unwrap().len(), 32);
    assert_eq!(
        MasterKey::from_base64(encoded)
            .unwrap()
            .to_base64()
            .as_str(),
        encoded
    );
    assert_ne!(key, stdout(&second));
}

#[test]
fn init_creates_an_empty_commented_file_that_validates() {
    let project = Project::new();

    project.init("development");
    let validate = project.run(&["validate", "--app", "myapp", "--env", "development"]);

    assert!(project.bytes(DEVELOPMENT).starts_with(b"credseal-v1\n"));
    assert_eq!(
        project.load::<BTreeMap<String, String>>("development"),
        BTreeMap::new()
    );
    assert!(validate.status.success());
    assert_eq!(stdout(&validate), format!("{DEVELOPMENT} is valid\n"));
}

#[test]
fn init_uses_the_given_directory() {
    let project = Project::new();

    let output = project.run(&[
        "init",
        "--app",
        "myapp",
        "--env",
        "staging",
        "--directory",
        "secrets",
    ]);

    assert!(output.status.success());
    assert!(project.path("secrets/staging.toml.enc").is_file());
}

#[test]
fn init_refuses_to_overwrite_an_existing_file() {
    let project = Project::new();
    project.init("development");
    let before = project.bytes(DEVELOPMENT);

    let output = project.run(&["init", "--app", "myapp", "--env", "development"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        format!("credseal: {DEVELOPMENT} already exists; change it with `credseal edit`\n")
    );
    assert_eq!(project.bytes(DEVELOPMENT), before);
}

#[test]
fn edit_creates_an_ignored_local_key_when_the_environment_key_is_absent() {
    let project = Project::new();
    let editor = project.editor("write", "printf 'token = \"local\"\\n' > \"$file\"");
    let output = project
        .command(&[
            "edit",
            "--app",
            "myapp",
            "-e",
            "development",
            "--editor",
            &quoted(&editor),
        ])
        .env_remove("CREDSEAL_MASTER_KEY")
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    let encoded = fs::read_to_string(project.path("config/credentials/development.key")).unwrap();
    let key = MasterKey::from_base64(&encoded).unwrap();
    let credentials: BTreeMap<String, String> = CredentialsFile::new(
        project.path(DEVELOPMENT),
        Context::new("myapp", "development").unwrap(),
    )
    .load(&key)
    .unwrap();
    assert_eq!(credentials.get("token").map(String::as_str), Some("local"));
    assert_eq!(
        fs::read_to_string(project.path("config/credentials/.gitignore")).unwrap(),
        "/development.key\n"
    );
    assert_eq!(
        fs::metadata(project.path("config/credentials/development.key"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[test]
fn the_application_and_environment_are_required_and_validated() {
    let project = Project::new();

    let missing = project.run(&["validate", "--app", "myapp"]);
    let invalid = project.run(&["validate", "--app", "My App", "--env", "development"]);

    assert_eq!(missing.status.code(), Some(2));
    assert!(stderr(&missing).contains("--environment <ENVIRONMENT>"));
    assert_eq!(invalid.status.code(), Some(1));
    assert_eq!(
        stderr(&invalid),
        "credseal: invalid application name: use 1 to 64 characters from a-z, 0-9, _ and -\n"
    );
}

#[test]
fn edit_saves_typed_credentials_and_removes_the_plaintext() {
    let project = Project::new();
    project.init("development");
    let editor = project.editor(
        "write",
        "printf '[stripe]\\nsecret_key = \"sk_test_51Hq\"\\n' > \"$file\"",
    );

    let output = project.edit("development", &editor);

    assert_eq!(stderr(&output), format!("Saved {DEVELOPMENT}\n"));
    assert!(output.status.success());
    let credentials: Credentials = project.load("development");
    assert_eq!(
        credentials.stripe.secret_key.expose_secret(),
        "sk_test_51Hq"
    );
    project.assert_no_plaintext_left();
}

fn assert_an_edit_survives_the_editor_sending(signal: &str) {
    let project = Project::new();
    project.init("development");
    let editor = project.editor(
        signal,
        &format!(
            "kill -{signal} $PPID\n\
             printf '[stripe]\\nsecret_key = \"sk_test_51Hq\"\\n' > \"$file\""
        ),
    );

    let output = project.edit("development", &editor);

    assert_eq!(stderr(&output), format!("Saved {DEVELOPMENT}\n"));
    assert!(output.status.success(), "{:?}", output.status);
    let credentials: Credentials = project.load("development");
    assert_eq!(
        credentials.stripe.secret_key.expose_secret(),
        "sk_test_51Hq"
    );
    project.assert_no_plaintext_left();
}

#[test]
fn an_interrupt_typed_in_the_editor_does_not_stop_the_edit() {
    assert_an_edit_survives_the_editor_sending("INT");
}

#[test]
fn a_quit_typed_in_the_editor_does_not_stop_the_edit() {
    assert_an_edit_survives_the_editor_sending("QUIT");
}

#[test]
fn an_editor_stopped_by_an_interrupt_leaves_the_credentials_unchanged() {
    let project = Project::new();
    project.init("development");
    let before = project.bytes(DEVELOPMENT);
    let editor = project.editor(
        "interrupted",
        "printf 'token = \"new\"\\n' > \"$file\"\nkill -INT $$\nsleep 5",
    );

    let output = project.edit("development", &editor);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        format!("credseal: the editor failed with signal: 2 (SIGINT); {UNCHANGED}\n")
    );
    assert_eq!(project.bytes(DEVELOPMENT), before);
    project.assert_no_plaintext_left();
}

#[test]
fn a_save_whose_directory_cannot_be_synced_says_that_it_was_saved() {
    let project = Project::new();
    project.init("development");
    let directory = project.path("config/credentials");
    let editor = project.editor(
        "write",
        "printf '[stripe]\\nsecret_key = \"sk_test_51Hq\"\\n' > \"$file\"",
    );
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o300)).unwrap();
    let directory_is_readable = fs::File::open(&directory).is_ok();

    let output = project.edit("development", &editor);

    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    if directory_is_readable {
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(stderr(&output), format!("Saved {DEVELOPMENT}\n"));
    } else {
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(
            stderr(&output),
            format!(
                "credseal: the file was saved to {DEVELOPMENT}, but syncing config/credentials failed: Permission denied (os error 13); the save may not survive a crash or power loss\n"
            )
        );
    }
    let credentials: Credentials = project.load("development");
    assert_eq!(
        credentials.stripe.secret_key.expose_secret(),
        "sk_test_51Hq"
    );
    project.assert_no_plaintext_left();
}

#[test]
fn the_editor_command_is_split_into_arguments_and_never_run_by_a_shell() {
    let project = Project::new();
    project.init("development");
    let log = project.path("editor.log");
    let editor = project.editor(
        "log arguments",
        "for argument; do printf 'argument %s\\n' \"$argument\" >> \"$EDITOR_LOG\"; done\n\
         printf 'key %s\\n' \"${CREDSEAL_MASTER_KEY-unset}\" >> \"$EDITOR_LOG\"\n\
         ls -ld \"$(dirname \"$file\")\" | cut -c1-10 >> \"$EDITOR_LOG\"\n\
         ls -l \"$file\" | cut -c1-10 >> \"$EDITOR_LOG\"",
    );
    let command = format!(
        "{} --mode 'two words' '$(touch pwned)' ';' \"a\\\"b\"",
        quoted(&editor)
    );

    let output = project
        .command(&[
            "edit",
            "--app",
            "myapp",
            "--env",
            "development",
            "--editor",
            &command,
        ])
        .env("EDITOR_LOG", &log)
        .output()
        .unwrap();

    assert!(output.status.success(), "{}", stderr(&output));
    let log = fs::read_to_string(log).unwrap();
    let lines: Vec<&str> = log.lines().collect();
    let plaintext = lines[5].strip_prefix("argument ").unwrap();
    assert_eq!(
        lines,
        [
            "argument --mode",
            "argument two words",
            "argument $(touch pwned)",
            "argument ;",
            "argument a\"b",
            &format!("argument {plaintext}"),
            "key unset",
            "drwx------",
            "-rw-------",
        ]
    );
    assert!(plaintext.ends_with("/development.toml"));
    assert!(plaintext.starts_with(project.temporary().to_str().unwrap()));
    assert!(!Path::new(plaintext).exists());
    assert!(!project.path("pwned").exists());
    project.assert_no_plaintext_left();
}

#[test]
fn edit_prefers_visual_then_editor() {
    let project = Project::new();
    project.init("development");
    let visual = project.editor("visual", "touch \"$MARKERS/visual\"");
    let editor = project.editor("editor", "touch \"$MARKERS/editor\"");
    let markers = project.path("markers");
    fs::create_dir(&markers).unwrap();

    let both = project
        .command(&["edit", "--app", "myapp", "--env", "development"])
        .env("VISUAL", quoted(&visual))
        .env("EDITOR", quoted(&editor))
        .env("MARKERS", &markers)
        .output()
        .unwrap();
    assert!(both.status.success());
    assert!(markers.join("visual").exists());
    assert!(!markers.join("editor").exists());

    let empty_visual = project
        .command(&["edit", "--app", "myapp", "--env", "development"])
        .env("VISUAL", "")
        .env("EDITOR", quoted(&editor))
        .env("MARKERS", &markers)
        .output()
        .unwrap();
    assert!(empty_visual.status.success());
    assert!(markers.join("editor").exists());
}

#[test]
fn an_edit_without_changes_keeps_the_exact_bytes() {
    let project = Project::new();
    project.init("development");
    let before = project.bytes(DEVELOPMENT);
    let editor = project.editor("noop", "true");

    let output = project.edit("development", &editor);

    assert!(output.status.success());
    assert_eq!(
        stderr(&output),
        format!("No changes; {DEVELOPMENT} is unchanged\n")
    );
    assert_eq!(project.bytes(DEVELOPMENT), before);
    project.assert_no_plaintext_left();
}

#[test]
fn a_failing_editor_leaves_the_credentials_unchanged() {
    let project = Project::new();
    project.init("development");
    let before = project.bytes(DEVELOPMENT);
    let editor = project.editor("fail", "printf 'token = \"new\"\\n' > \"$file\"\nexit 3");

    let output = project.edit("development", &editor);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        format!("credseal: the editor failed with exit status: 3; {UNCHANGED}\n")
    );
    assert_eq!(project.bytes(DEVELOPMENT), before);
    project.assert_no_plaintext_left();
}

#[test]
fn invalid_toml_is_rejected_without_showing_it() {
    let project = Project::new();
    project.init("development");
    let before = project.bytes(DEVELOPMENT);
    let editor = project.editor("break", "printf 'token = \"sk_live_SECRET\\n' > \"$file\"");

    let output = project.edit("development", &editor);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        format!(
            "credseal: edited credentials rejected: credentials are not valid TOML (line 1); {UNCHANGED}\n"
        )
    );
    assert_eq!(project.bytes(DEVELOPMENT), before);
    project.assert_no_plaintext_left();
}

#[test]
fn an_edited_file_replaced_by_a_symbolic_link_is_rejected() {
    let project = Project::new();
    project.init("development");
    let before = project.bytes(DEVELOPMENT);
    let editor = project.editor("link", "rm \"$file\"\nln -s /etc/hosts \"$file\"");

    let output = project.edit("development", &editor);

    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).ends_with(&format!(
        "development.toml is not a regular file; {UNCHANGED}\n"
    )));
    assert_eq!(project.bytes(DEVELOPMENT), before);
    project.assert_no_plaintext_left();
}

#[test]
fn a_second_edit_of_the_same_file_is_refused_while_other_environments_stay_free() {
    let project = Project::new();
    project.init("development");
    let started = project.path("started");
    let release = project.path("release");
    let slow = project.editor(
        "slow",
        "touch \"$STARTED\"\n\
         waited=0\n\
         while [ ! -e \"$RELEASE\" ]; do\n\
           [ $waited -lt 600 ] || exit 1\n\
           sleep 0.05\n\
           waited=$((waited + 1))\n\
         done",
    );
    let child = project
        .command(&[
            "edit",
            "--app",
            "myapp",
            "--env",
            "development",
            "--editor",
            &quoted(&slow),
        ])
        .env("STARTED", &started)
        .env("RELEASE", &release)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let first = HeldEdit {
        child: Some(child),
        release,
    };
    wait_for(&started);

    let noop = project.editor("noop", "true");
    let second = project.edit("development", &noop);
    let second_init = project.run(&["init", "--app", "myapp", "--env", "development"]);
    let production = project.run(&["init", "--app", "myapp", "--env", "production"]);
    let first = first.finish();

    assert_eq!(second.status.code(), Some(1));
    assert_eq!(
        stderr(&second),
        format!("credseal: {DEVELOPMENT} is locked by another credseal process\n")
    );
    assert_eq!(stderr(&second_init), stderr(&second));
    assert!(production.status.success());
    assert!(first.status.success(), "{}", stderr(&first));
    let lock = fs::metadata(project.path(&format!("{DEVELOPMENT}.lock"))).unwrap();
    assert_eq!(lock.permissions().mode() & 0o777, 0o600);
    project.assert_no_plaintext_left();
}

#[test]
fn a_change_made_during_the_edit_is_kept_and_the_edit_discarded() {
    let project = Project::new();
    project.init("development");
    let editor = project.editor(
        "race",
        "printf 'token = \"mine\"\\n' > \"$file\"\nprintf 'theirs' > \"$CREDENTIALS\"",
    );

    let output = project
        .command(&[
            "edit",
            "--app",
            "myapp",
            "--env",
            "development",
            "--editor",
            &quoted(&editor),
        ])
        .env("CREDENTIALS", project.path(DEVELOPMENT))
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        format!("credseal: {DEVELOPMENT} changed during the edit; the change was kept and the edits were discarded\n")
    );
    assert_eq!(project.bytes(DEVELOPMENT), b"theirs");
    project.assert_no_plaintext_left();
}

#[test]
fn edit_with_a_wrong_key_never_opens_the_editor() {
    let project = Project::new();
    project.init("development");
    let before = project.bytes(DEVELOPMENT);
    let editor = project.editor("marker", "touch \"$MARKER\"");

    let output = project
        .command(&[
            "edit",
            "--app",
            "myapp",
            "--env",
            "development",
            "--editor",
            &quoted(&editor),
        ])
        .env("CREDSEAL_MASTER_KEY", OTHER_KEY_BASE64)
        .env("MARKER", project.path("opened"))
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        "credseal: cannot decrypt credentials: wrong key, wrong application or environment, or modified file\n"
    );
    assert!(!project.path("opened").exists());
    assert_eq!(project.bytes(DEVELOPMENT), before);
}

#[test]
fn validate_reports_each_failure_without_secrets() {
    let project = Project::new();
    project.init("development");
    let editor = project.editor(
        "write",
        "printf 'token = \"sk_live_SECRET\"\\n' > \"$file\"",
    );
    assert!(project.edit("development", &editor).status.success());
    fs::create_dir_all(project.path("linked")).unwrap();
    std::os::unix::fs::symlink(
        project.path(DEVELOPMENT),
        project.path("linked/development.toml.enc"),
    )
    .unwrap();

    let cases: [(&[&str], Option<&str>, String); 6] = [
        (&["--app", "myapp", "--env", "development"], None, format!("{DEVELOPMENT} is valid\n")),
        (
            &["--app", "myapp", "--env", "production"],
            None,
            "credseal: cannot open config/credentials/production.toml.enc: No such file or directory (os error 2)\n".into(),
        ),
        (
            &["--app", "myapp", "--env", "development"],
            Some(OTHER_KEY_BASE64),
            "credseal: cannot decrypt credentials: wrong key, wrong application or environment, or modified file\n".into(),
        ),
        (
            &["--app", "otherapp", "--env", "development"],
            None,
            "credseal: cannot decrypt credentials: wrong key, wrong application or environment, or modified file\n".into(),
        ),
        (
            &["--app", "myapp", "--env", "development"],
            Some("c2hvcnQ="),
            "credseal: the master key is not Base64 of exactly 32 bytes\n".into(),
        ),
        (
            &["--app", "myapp", "--env", "development", "--directory", "linked"],
            None,
            "credseal: linked/development.toml.enc is not a regular file\n".into(),
        ),
    ];
    for (arguments, key, expected) in cases {
        let mut command = project.command(&[&["validate"], arguments].concat());
        if let Some(key) = key {
            command.env("CREDSEAL_MASTER_KEY", key);
        }
        let output = command.output().unwrap();
        let printed = format!("{}{}", stdout(&output), stderr(&output));
        assert_eq!(printed, expected, "{arguments:?}");
        assert_eq!(output.status.success(), expected.ends_with("is valid\n"));
        assert!(!printed.contains("SECRET"));
    }
}

const MANIFEST: &str = "[package]\nname = \"myapp\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";
const STAGING: &str = "config/credentials/staging.toml.enc";

#[test]
fn the_environment_is_given_with_e_environment_or_env() {
    let project = Project::new();

    let init = project.run(&["init", "--app", "myapp", "-e", "staging"]);
    let long = project.run(&["validate", "--app", "myapp", "--environment", "staging"]);
    let alias = project.run(&["validate", "--app", "myapp", "--env", "staging"]);

    assert_eq!(stderr(&init), format!("Created {STAGING}\n"));
    assert_eq!(stdout(&long), format!("{STAGING} is valid\n"));
    assert_eq!(stdout(&alias), format!("{STAGING} is valid\n"));
}

#[test]
fn in_a_rust_package_edit_creates_the_environment_and_saves_what_the_user_typed() {
    let project = Project::new();
    project.manifest(MANIFEST);
    let editor = project.editor(
        "write",
        "printf '[stripe]\\nsecret_key = \"sk_test_51Hq\"\\n' > \"$file\"",
    );

    let output = project
        .command(&["edit", "-e", "staging"])
        .env("EDITOR", quoted(&editor))
        .output()
        .unwrap();

    assert_eq!(stderr(&output), format!("Created {STAGING}\n"));
    assert!(output.status.success());
    let credentials: Credentials = project.load("staging");
    assert_eq!(
        credentials.stripe.secret_key.expose_secret(),
        "sk_test_51Hq"
    );
    project.assert_no_plaintext_left();
}

#[test]
fn credentials_edit_is_another_name_for_edit() {
    let project = Project::new();
    project.manifest(MANIFEST);
    let editor = project.editor("write", "printf 'token = \"t0k3n\"\\n' > \"$file\"");

    let output = project.run(&[
        "credentials:edit",
        "-e",
        "staging",
        "--editor",
        &quoted(&editor),
    ]);

    assert_eq!(stderr(&output), format!("Created {STAGING}\n"));
    assert_eq!(
        project.load::<BTreeMap<String, String>>("staging"),
        BTreeMap::from([("token".to_owned(), "t0k3n".to_owned())])
    );
}

#[test]
fn a_new_environment_left_unchanged_in_the_editor_is_created_from_the_template() {
    let project = Project::new();
    project.manifest(MANIFEST);
    let log = project.path("seen.toml");
    let editor = project.editor("copy", "cp \"$file\" \"$SEEN\"");

    let output = project
        .command(&["edit", "-e", "staging", "--editor", &quoted(&editor)])
        .env("SEEN", &log)
        .output()
        .unwrap();

    assert_eq!(stderr(&output), format!("Created {STAGING}\n"));
    assert_eq!(
        fs::read_to_string(log).unwrap(),
        "# Credentials for myapp in staging, as TOML.\n\
         # Example:\n\
         #\n\
         # [stripe]\n\
         # secret_key = \"sk_live_...\"\n"
    );
    assert_eq!(
        project.load::<BTreeMap<String, String>>("staging"),
        BTreeMap::new()
    );
    project.assert_no_plaintext_left();
}

#[test]
fn a_failed_edit_of_a_new_environment_creates_no_credentials_file() {
    let cases = [
        (
            "printf 'token = \"new\"\\n' > \"$file\"\nexit 3",
            format!("credseal: the editor failed with exit status: 3; {UNCHANGED}\n"),
        ),
        (
            "printf 'token = \"sk_live_SECRET\\n' > \"$file\"",
            format!(
                "credseal: edited credentials rejected: credentials are not valid TOML (line 1); {UNCHANGED}\n"
            ),
        ),
    ];
    for (body, expected) in cases {
        let project = Project::new();
        project.manifest(MANIFEST);
        let editor = project.editor("fail", body);

        let output = project.run(&["edit", "-e", "staging", "--editor", &quoted(&editor)]);

        assert_eq!(output.status.code(), Some(1));
        assert_eq!(stderr(&output), expected);
        assert!(!project.path(STAGING).exists());
        project.assert_no_plaintext_left();
    }
}

#[test]
fn an_explicit_application_overrides_the_package_name() {
    let project = Project::new();
    project.manifest(MANIFEST);

    let init = project.run(&["init", "--app", "otherapp", "-e", "staging"]);
    let explicit = project.run(&["validate", "--app", "otherapp", "-e", "staging"]);
    let inferred = project.run(&["validate", "-e", "staging"]);

    assert!(init.status.success());
    assert_eq!(stdout(&explicit), format!("{STAGING} is valid\n"));
    assert_eq!(
        stderr(&inferred),
        "credseal: cannot decrypt credentials: wrong key, wrong application or environment, or modified file\n"
    );
}

#[test]
fn without_app_a_directory_without_a_package_manifest_is_refused_safely() {
    const PASS: &str = "; pass --app <APPLICATION>\n";
    let cases: [(Option<&str>, String); 5] = [
        (None, "there is no Cargo.toml".into()),
        (
            Some("[package]\nname = \"sk_live_SECRET\n"),
            "Cargo.toml is not valid TOML".into(),
        ),
        (
            Some("[workspace]\nmembers = [\"sk_live_SECRET\"]\n"),
            "Cargo.toml has no [package] name".into(),
        ),
        (
            Some("[package]\nname = \"SK_LIVE_SECRET\"\n"),
            "the package name in Cargo.toml is not 1 to 64 characters from a-z, 0-9, _ and -"
                .into(),
        ),
        (Some("\u{0}\u{ff}"), "Cargo.toml is not valid TOML".into()),
    ];
    for (manifest, reason) in cases {
        let project = Project::new();
        if let Some(manifest) = manifest {
            project.manifest(manifest);
        }
        let editor = project.editor("marker", "touch \"$MARKER\"");

        let output = project
            .command(&["edit", "-e", "staging", "--editor", &quoted(&editor)])
            .env("MARKER", project.path("opened"))
            .output()
            .unwrap();

        assert_eq!(output.status.code(), Some(1));
        assert_eq!(
            stderr(&output),
            format!("credseal: cannot take the application name from the current directory: {reason}{PASS}")
        );
        assert!(!project.path("opened").exists());
        assert!(!project.path("config").exists());
    }
}

#[test]
fn a_package_manifest_that_is_a_symbolic_link_is_refused() {
    let project = Project::new();
    fs::write(project.path("elsewhere.toml"), MANIFEST).unwrap();
    std::os::unix::fs::symlink(project.path("elsewhere.toml"), project.path("Cargo.toml")).unwrap();

    let output = project.run(&["validate", "-e", "staging"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        "credseal: cannot take the application name from the current directory: Cargo.toml is not a regular file; pass --app <APPLICATION>\n"
    );
}

#[test]
fn edit_with_a_wrong_key_never_opens_the_editor_for_an_inferred_application() {
    let project = Project::new();
    project.manifest(MANIFEST);
    project.init("staging");
    let before = project.bytes(STAGING);
    let editor = project.editor("marker", "touch \"$MARKER\"");

    let output = project
        .command(&["edit", "-e", "staging", "--editor", &quoted(&editor)])
        .env("CREDSEAL_MASTER_KEY", OTHER_KEY_BASE64)
        .env("MARKER", project.path("opened"))
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        "credseal: cannot decrypt credentials: wrong key, wrong application or environment, or modified file\n"
    );
    assert!(!project.path("opened").exists());
    assert_eq!(project.bytes(STAGING), before);
}

fn without_environment_key(project: &Project, command: &str, environment: &str) -> Output {
    project
        .command(&[command, "--app", "myapp", "-e", environment])
        .env_remove("CREDSEAL_MASTER_KEY")
        .output()
        .unwrap()
}

#[test]
fn local_keys_are_reused_and_separate_for_each_environment() {
    let project = Project::new();
    assert!(without_environment_key(&project, "init", "development")
        .status
        .success());
    let first = project.bytes("config/credentials/development.key");
    assert!(without_environment_key(&project, "validate", "development")
        .status
        .success());
    let editor = project.editor("unchanged", "exit 0");
    let output = project
        .command(&[
            "edit",
            "--app",
            "myapp",
            "-e",
            "development",
            "--editor",
            &quoted(&editor),
        ])
        .env_remove("CREDSEAL_MASTER_KEY")
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(first, project.bytes("config/credentials/development.key"));
    assert!(without_environment_key(&project, "init", "staging")
        .status
        .success());
    assert_ne!(first, project.bytes("config/credentials/staging.key"));
    assert!(without_environment_key(&project, "validate", "staging")
        .status
        .success());
}

#[test]
fn validate_never_generates_a_key_and_missing_keys_for_existing_credentials_are_not_replaced() {
    let project = Project::new();
    let fresh = without_environment_key(&project, "validate", "development");
    assert_eq!(fresh.status.code(), Some(1));
    assert_eq!(stderr(&fresh), "credseal: no master key: set CREDSEAL_MASTER_KEY or restore config/credentials/development.key\n");
    assert!(!project.path("config").exists());
    project.init("development");
    let before = project.bytes(DEVELOPMENT);
    let editor = project.editor("never", "touch opened");
    let output = project
        .command(&[
            "edit",
            "--app",
            "myapp",
            "-e",
            "development",
            "--editor",
            &quoted(&editor),
        ])
        .env_remove("CREDSEAL_MASTER_KEY")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(project.bytes(DEVELOPMENT), before);
    assert!(!project.path("config/credentials/development.key").exists());
    assert!(!project.path("opened").exists());
}

#[test]
fn an_explicit_environment_key_takes_precedence_and_an_empty_key_does_not_fall_back() {
    let project = Project::new();
    project.init("development");
    fs::write(
        project.path("config/credentials/development.key"),
        "malformed",
    )
    .unwrap();
    assert!(project
        .run(&["validate", "--app", "myapp", "-e", "development"])
        .status
        .success());
    let empty = project
        .command(&["validate", "--app", "myapp", "-e", "development"])
        .env("CREDSEAL_MASTER_KEY", "")
        .output()
        .unwrap();
    assert_eq!(
        stderr(&empty),
        "credseal: the master key is not Base64 of exactly 32 bytes\n"
    );
    let malformed = without_environment_key(&project, "validate", "development");
    assert_eq!(
        stderr(&malformed),
        "credseal: the master key is not Base64 of exactly 32 bytes\n"
    );
    assert_eq!(
        project.bytes("config/credentials/development.key"),
        b"malformed"
    );
}

#[test]
fn a_generated_key_survives_an_editor_failure_without_creating_credentials() {
    let project = Project::new();
    let editor = project.editor("fail", "exit 42");
    let output = project
        .command(&[
            "edit",
            "--app",
            "myapp",
            "-e",
            "development",
            "--editor",
            &quoted(&editor),
        ])
        .env_remove("CREDSEAL_MASTER_KEY")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(!project.path(DEVELOPMENT).exists());
    MasterKey::from_base64(
        &fs::read_to_string(project.path("config/credentials/development.key")).unwrap(),
    )
    .unwrap();
    project.assert_no_plaintext_left();
}

fn git(project: &Project, arguments: &[&str]) -> Output {
    Command::new("git")
        .args(arguments)
        .current_dir(project.root.path())
        .output()
        .unwrap()
}

#[test]
fn git_ignores_generated_keys_even_after_negated_rules_and_reuses_the_final_rule() {
    let project = Project::new();
    assert!(git(&project, &["init", "-q"]).status.success());
    fs::create_dir_all(project.path("config/credentials")).unwrap();
    fs::write(project.path(".gitignore"), "!*.key\n").unwrap();
    fs::write(
        project.path("config/credentials/.gitignore"),
        "# keep this\n*.key\n!development.key",
    )
    .unwrap();
    let output = without_environment_key(&project, "init", "development");
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        fs::read_to_string(project.path("config/credentials/.gitignore")).unwrap(),
        "# keep this\n*.key\n!development.key\n/development.key\n"
    );
    assert!(git(
        &project,
        &["check-ignore", "-q", "config/credentials/development.key"]
    )
    .status
    .success());
    fs::remove_file(project.path(DEVELOPMENT)).unwrap();
    fs::remove_file(project.path("config/credentials/development.key")).unwrap();
    assert!(without_environment_key(&project, "init", "development")
        .status
        .success());
    assert_eq!(
        fs::read_to_string(project.path("config/credentials/.gitignore"))
            .unwrap()
            .matches("/development.key")
            .count(),
        1
    );
}

#[test]
fn tracked_keys_are_refused_even_when_deleted_from_the_worktree() {
    for deleted in [false, true] {
        let project = Project::new();
        assert!(git(&project, &["init", "-q"]).status.success());
        fs::create_dir_all(project.path("config/credentials")).unwrap();
        fs::write(
            project.path("config/credentials/development.key"),
            KEY_BASE64,
        )
        .unwrap();
        assert!(
            git(&project, &["add", "config/credentials/development.key"])
                .status
                .success()
        );
        if deleted {
            fs::remove_file(project.path("config/credentials/development.key")).unwrap();
        }
        let output = without_environment_key(&project, "init", "development");
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(stderr(&output), "credseal: refusing local key config/credentials/development.key because Git tracks it; remove it from the index and rotate any exposed key\n");
        assert!(!project.path(DEVELOPMENT).exists());
        assert_eq!(
            project.path("config/credentials/development.key").exists(),
            !deleted
        );
    }
}

#[test]
fn local_key_and_ignore_symlinks_are_refused_before_secret_creation() {
    for name in ["development.key", ".gitignore"] {
        let project = Project::new();
        fs::create_dir_all(project.path("config/credentials")).unwrap();
        fs::write(project.path("untouched"), "keep").unwrap();
        std::os::unix::fs::symlink(
            project.path("untouched"),
            project.path(&format!("config/credentials/{name}")),
        )
        .unwrap();
        let output = without_environment_key(&project, "init", "development");
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(
            stderr(&output),
            format!("credseal: config/credentials/{name} is not a regular file\n")
        );
        assert_eq!(project.bytes("untouched"), b"keep");
        assert!(!project.path(DEVELOPMENT).exists());
    }
}

#[test]
fn a_broken_git_repository_prevents_local_key_creation() {
    let project = Project::new();
    fs::create_dir(project.path(".git")).unwrap();
    let output = without_environment_key(&project, "init", "development");
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        "credseal: cannot verify Git protection for config/credentials\n"
    );
    assert!(!project.path("config/credentials/development.key").exists());
    assert!(!project.path(DEVELOPMENT).exists());
}

#[test]
fn oversized_and_non_regular_local_key_files_are_refused() {
    let project = Project::new();
    fs::create_dir_all(project.path("config/credentials")).unwrap();
    let key = project.path("config/credentials/development.key");
    fs::write(&key, vec![b'A'; 129]).unwrap();
    let oversized = without_environment_key(&project, "init", "development");
    assert_eq!(oversized.status.code(), Some(1));
    assert_eq!(
        stderr(&oversized),
        "credseal: config/credentials/development.key is larger than the limit of 128 bytes\n"
    );
    fs::remove_file(&key).unwrap();
    fs::create_dir(&key).unwrap();
    let directory = without_environment_key(&project, "init", "development");
    assert_eq!(
        stderr(&directory),
        "credseal: config/credentials/development.key is not a regular file\n"
    );
    assert!(!project.path(DEVELOPMENT).exists());
}

#[test]
fn concurrent_environments_keep_both_gitignore_rules_and_private_keys() {
    let project = Project::new();
    assert!(git(&project, &["init", "-q"]).status.success());
    let children: Vec<_> = ["development", "staging", "production"]
        .into_iter()
        .map(|environment| {
            project
                .command(&["init", "--app", "myapp", "-e", environment])
                .env_remove("CREDSEAL_MASTER_KEY")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    for child in children {
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{}", stderr(&output));
    }
    let ignore = fs::read_to_string(project.path("config/credentials/.gitignore")).unwrap();
    for environment in ["development", "staging", "production"] {
        assert!(ignore
            .lines()
            .any(|line| line == format!("/{environment}.key")));
        assert!(without_environment_key(&project, "validate", environment)
            .status
            .success());
        assert!(git(
            &project,
            &[
                "check-ignore",
                "-q",
                &format!("config/credentials/{environment}.key")
            ]
        )
        .status
        .success());
    }
}
