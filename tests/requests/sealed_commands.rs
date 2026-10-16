use std::{
    collections::BTreeMap,
    env,
    process::{Command, Output},
};

const HELPER: &str = env!("CARGO_BIN_EXE_keysend-credentials");
const UNLOCK_KEY: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";
const OTHER_UNLOCK_KEY: &str = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=";
const SEALED: &str = "tests/fixtures/credentials/sealed";
const STALE: [(&str, &str); 8] = [
    ("GOOGLE_CLIENT_SECRET", "stale-google-secret"),
    ("KEYSEND_MASTER_KEY", "stale-data-key"),
    ("KAMAL_REGISTRY_PASSWORD", "stale-registry-secret"),
    ("R2_SECRET_ACCESS_KEY", "stale-r2-secret"),
    ("AGE_RECIPIENT", "stale-age-recipient"),
    ("KEYSEND_CREDENTIALS_FILE", "stale-file"),
    (
        "RCLONE_CONFIG_OLD_SECRET_ACCESS_KEY",
        "stale-old-remote-secret",
    ),
    ("UNRELATED", "kept"),
];

fn helper(arguments: &[&str], unlock_key: Option<&str>, extra: &[(&str, &str)]) -> Output {
    let mut command = Command::new(HELPER);
    command
        .args(arguments)
        .env_clear()
        .env("PATH", env::var("PATH").unwrap())
        .envs(STALE)
        .envs(extra.iter().copied());
    if let Some(unlock_key) = unlock_key {
        command.env("CREDSEAL_MASTER_KEY", unlock_key);
    }
    command.output().unwrap()
}

fn exec(
    scope: &str,
    directory: &str,
    unlock_key: Option<&str>,
    extra: &[(&str, &str)],
    program: &[&str],
) -> Output {
    let mut arguments = vec![
        "exec",
        "--scope",
        scope,
        "--environment",
        "test",
        "--directory",
        directory,
        "--",
    ];
    arguments.extend(program);
    helper(&arguments, unlock_key, extra)
}

fn child_environment(output: &Output) -> BTreeMap<String, String> {
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout.clone())
        .unwrap()
        .lines()
        .filter_map(|line| line.split_once('='))
        .filter(|(name, _)| *name != "PATH" && !name.starts_with("__"))
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect()
}

fn refusal(output: &Output) -> String {
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    String::from_utf8(output.stderr.clone()).unwrap()
}

fn expected(values: &[(&str, &str)]) -> BTreeMap<String, String> {
    values
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect()
}

#[test]
fn a_release_gets_only_the_deploy_credentials_and_the_unlock_key_for_the_container() {
    let output = exec("deploy", SEALED, Some(UNLOCK_KEY), &[], &["env"]);
    assert_eq!(
        child_environment(&output),
        expected(&[
            ("CI_KNOWN_HOSTS", "sealed-known-hosts"),
            ("CI_SSH_KEY", "sealed-ssh-secret"),
            ("CLOUDFLARE_CACHE_TOKEN", "sealed-cache-secret"),
            ("CLOUDFLARE_ORIGIN_CERTIFICATE", "sealed-certificate"),
            ("CLOUDFLARE_ORIGIN_KEY", "sealed-origin-secret"),
            ("CREDSEAL_MASTER_KEY", UNLOCK_KEY),
            ("KAMAL_REGISTRY_PASSWORD", "sealed-registry-secret"),
            (
                "KEYSEND_CREDENTIALS_FILE",
                "tests/fixtures/credentials/sealed/test.toml.enc"
            ),
            ("POSTDEPLOY_INGEST_KEY", "sealed-postdeploy-secret"),
            ("STRIPE_SECRET_KEY", "sk_test_sealed-stripe-secret"),
            ("STRIPE_WEBHOOK_SECRET", "whsec_sealed-stripe-secret"),
            ("UNRELATED", "kept"),
        ])
    );
}

#[test]
fn a_backup_gets_only_the_age_recipient_and_the_keysend_r2_remote() {
    let output = exec(
        "backup",
        SEALED,
        Some(UNLOCK_KEY),
        &[("RCLONE_REMOTE", "keysend_r2:keysend-backups")],
        &["env"],
    );
    assert_eq!(
        child_environment(&output),
        expected(&[
            ("AGE_RECIPIENT", "sealed-age-recipient"),
            ("RCLONE_CONFIG_KEYSEND_R2_ACCESS_KEY_ID", "sealed-r2-access"),
            (
                "RCLONE_CONFIG_KEYSEND_R2_ENDPOINT",
                "https://example.r2.cloudflarestorage.com"
            ),
            ("RCLONE_CONFIG_KEYSEND_R2_ENV_AUTH", "false"),
            ("RCLONE_CONFIG_KEYSEND_R2_NO_CHECK_BUCKET", "true"),
            ("RCLONE_CONFIG_KEYSEND_R2_PROVIDER", "Cloudflare"),
            ("RCLONE_CONFIG_KEYSEND_R2_REGION", "auto"),
            (
                "RCLONE_CONFIG_KEYSEND_R2_SECRET_ACCESS_KEY",
                "sealed-r2-secret"
            ),
            ("RCLONE_CONFIG_KEYSEND_R2_TYPE", "s3"),
            ("RCLONE_REMOTE", "keysend_r2:keysend-backups"),
            ("UNRELATED", "kept"),
        ])
    );
}

#[test]
fn a_backup_to_another_rclone_remote_does_not_run() {
    for remote in [
        None,
        Some("old_remote:keysend-backups"),
        Some("keysend_r2:"),
    ] {
        let extra: Vec<_> = remote
            .map(|remote| ("RCLONE_REMOTE", remote))
            .into_iter()
            .collect();
        let output = exec(
            "backup",
            SEALED,
            Some(UNLOCK_KEY),
            &extra,
            &["sh", "-c", "echo ran"],
        );
        assert_eq!(
            refusal(&output),
            "keysend-credentials: RCLONE_REMOTE must be keysend_r2:BUCKET for the sealed backup\n"
        );
    }
}

#[test]
fn a_command_does_not_run_without_the_right_unlock_key_or_with_an_unreadable_file() {
    let decrypt =
        "cannot decrypt credentials: wrong key, wrong application or environment, or modified file";
    let cases = [
        (
            None,
            SEALED,
            format!("Cannot load the credentials in {SEALED}/test.toml.enc: CREDSEAL_MASTER_KEY is not set"),
        ),
        (
            Some(""),
            SEALED,
            format!("Cannot load the credentials in {SEALED}/test.toml.enc: the master key is not Base64 of exactly 32 bytes"),
        ),
        (
            Some(OTHER_UNLOCK_KEY),
            SEALED,
            format!("Cannot load the credentials in {SEALED}/test.toml.enc: {decrypt}"),
        ),
        (
            Some(UNLOCK_KEY),
            "tests/fixtures/credentials/unknown-field",
            "Cannot load the credentials in tests/fixtures/credentials/unknown-field/test.toml.enc: credentials do not match the expected type (line 2)".into(),
        ),
    ];
    for (unlock_key, directory, message) in cases {
        let output = exec(
            "deploy",
            directory,
            unlock_key,
            &[],
            &["sh", "-c", "echo ran"],
        );
        let error = refusal(&output);
        assert_eq!(error, format!("keysend-credentials: {message}\n"));
        assert!(!error.contains("sealed-"), "{error}");
        assert!(!error.contains("misspelled"), "{error}");
    }
}

#[test]
fn a_command_does_not_run_when_its_scope_lacks_a_value_or_a_value_has_a_nul_byte() {
    let cases = [
        (
            "deploy",
            "tests/fixtures/credentials/backup-only",
            "The credentials have no kamal_registry_password, which this scope needs",
        ),
        (
            "backup",
            "tests/fixtures/credentials/nul-byte",
            "r2_secret_access_key has a NUL byte",
        ),
    ];
    for (scope, directory, message) in cases {
        let output = exec(
            scope,
            directory,
            Some(UNLOCK_KEY),
            &[("RCLONE_REMOTE", "keysend_r2:keysend-backups")],
            &["sh", "-c", "echo ran"],
        );
        assert_eq!(
            refusal(&output),
            format!("keysend-credentials: {message}\n")
        );
    }
}

#[test]
fn a_command_keeps_its_exit_status() {
    let output = exec(
        "deploy",
        SEALED,
        Some(UNLOCK_KEY),
        &[],
        &["sh", "-c", "exit 7"],
    );
    assert_eq!(output.status.code(), Some(7));
}

#[test]
fn the_operator_sees_what_a_sealed_file_is_ready_for_and_no_value() {
    let status = |directory| {
        helper(
            &["status", "--environment", "test", "--directory", directory],
            Some(UNLOCK_KEY),
            &[],
        )
    };
    let complete = status(SEALED);
    assert_eq!(complete.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(complete.stdout).unwrap(),
        "{\"payments_ready\":true,\"postdeploy_ready\":true,\"deploy_ready\":true,\"backup_ready\":true}\n"
    );
    let backup_only = status("tests/fixtures/credentials/backup-only");
    assert_eq!(
        String::from_utf8(backup_only.stdout).unwrap(),
        "{\"payments_ready\":false,\"postdeploy_ready\":false,\"deploy_ready\":false,\"backup_ready\":true}\n"
    );
    let production = helper(
        &[
            "status",
            "--environment",
            "production",
            "--directory",
            "tests/fixtures/credentials/without-data-key",
        ],
        Some(UNLOCK_KEY),
        &[],
    );
    assert_eq!(
        refusal(&production),
        "keysend-credentials: KEYSEND_MASTER_KEY is required in production\n"
    );
}
