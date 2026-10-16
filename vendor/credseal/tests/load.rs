//! Loading credentials through the library.

mod support;

use std::collections::BTreeMap;
use std::path::PathBuf;

use credseal::{Context, CredentialsFile, Error, MasterKey, SecretString};
use serde::{Deserialize, Deserializer};
use support::{file_from_binary, sealed_binary, sealed_file, KEY_BASE64, OTHER_KEY_BASE64};
use tempfile::TempDir;

const STRIPE: &[u8] = b"[stripe]\nsecret_key = \"sk_test_51Hq\"\npublishable_key = \"pk_test_9\"\n";

#[derive(Debug, Deserialize)]
struct Credentials {
    stripe: Stripe,
}

#[derive(Debug, Deserialize)]
struct Stripe {
    secret_key: SecretString,
    publishable_key: String,
}

fn key() -> MasterKey {
    MasterKey::from_base64(KEY_BASE64).unwrap()
}

fn context(application: &str, environment: &str) -> Context {
    Context::new(application, environment).unwrap()
}

fn write(directory: &TempDir, contents: &[u8]) -> PathBuf {
    let path = directory.path().join("development.toml.enc");
    std::fs::write(&path, contents).unwrap();
    path
}

fn load_bytes<T: serde::de::DeserializeOwned>(contents: &[u8]) -> Result<T, Error> {
    let directory = TempDir::new().unwrap();
    let path = write(&directory, contents);
    CredentialsFile::new(path, context("myapp", "development")).load(&key())
}

#[test]
fn an_application_reads_a_typed_secret_from_a_file_in_the_documented_format() {
    let credentials: Credentials =
        load_bytes(&sealed_file("myapp", "development", STRIPE)).unwrap();

    assert_eq!(
        credentials.stripe.secret_key.expose_secret(),
        "sk_test_51Hq"
    );
    assert_eq!(credentials.stripe.publishable_key, "pk_test_9");
    assert_eq!(
        format!("{credentials:?}"),
        "Credentials { stripe: Stripe { secret_key: SecretString([REDACTED]), publishable_key: \"pk_test_9\" } }"
    );
}

#[test]
fn a_file_without_the_trailing_newline_still_loads() {
    let mut file = sealed_file("myapp", "development", STRIPE);
    assert_eq!(file.pop(), Some(b'\n'));

    let credentials: Credentials = load_bytes(&file).unwrap();

    assert_eq!(
        credentials.stripe.secret_key.expose_secret(),
        "sk_test_51Hq"
    );
}

#[test]
fn a_wrong_key_cannot_decrypt() {
    let directory = TempDir::new().unwrap();
    let path = write(&directory, &sealed_file("myapp", "development", STRIPE));
    let other_key = MasterKey::from_base64(OTHER_KEY_BASE64).unwrap();

    let error = CredentialsFile::new(path, context("myapp", "development"))
        .load::<Credentials>(&other_key)
        .unwrap_err();

    assert!(matches!(error, Error::Decrypt));
    assert_eq!(
        error.to_string(),
        "cannot decrypt credentials: wrong key, wrong application or environment, or modified file"
    );
}

#[test]
fn a_file_of_another_application_or_environment_cannot_decrypt() {
    let directory = TempDir::new().unwrap();
    let path = write(&directory, &sealed_file("myapp", "development", STRIPE));

    for (application, environment) in [("otherapp", "development"), ("myapp", "production")] {
        let error = CredentialsFile::new(&path, context(application, environment))
            .load::<Credentials>(&key())
            .unwrap_err();
        assert!(
            matches!(error, Error::Decrypt),
            "{application} {environment}"
        );
    }
}

#[test]
fn names_are_length_prefixed_so_shifting_a_character_between_them_cannot_decrypt() {
    let file = sealed_file("ab", "c", b"token = \"x\"\n");
    let directory = TempDir::new().unwrap();
    let path = write(&directory, &file);

    let error = CredentialsFile::new(path, context("a", "bc"))
        .load::<BTreeMap<String, String>>(&key())
        .unwrap_err();

    assert!(matches!(error, Error::Decrypt));
}

#[test]
fn flipping_one_bit_of_the_nonce_ciphertext_or_tag_cannot_decrypt() {
    let binary = sealed_binary("myapp", "development", STRIPE);
    for index in [0, 11, 12, binary.len() / 2, binary.len() - 1] {
        let mut tampered = binary.clone();
        tampered[index] ^= 0x01;

        let error = load_bytes::<Credentials>(&file_from_binary(&tampered)).unwrap_err();

        assert!(matches!(error, Error::Decrypt), "byte {index}");
    }
}

#[test]
fn a_truncated_file_is_rejected() {
    let binary = sealed_binary("myapp", "development", STRIPE);

    let shorter = load_bytes::<Credentials>(&file_from_binary(&binary[..binary.len() - 3]));
    let header_only = load_bytes::<Credentials>(b"credseal-v1\n");
    let too_short = load_bytes::<Credentials>(&file_from_binary(&binary[..27]));

    assert!(matches!(shorter, Err(Error::Decrypt)));
    assert!(matches!(header_only, Err(Error::Malformed)));
    assert!(matches!(too_short, Err(Error::Malformed)));
}

#[test]
fn another_format_version_or_malformed_text_is_rejected() {
    let mut version_two = sealed_file("myapp", "development", STRIPE);
    version_two[10] = b'2';
    let mut broken_base64 = sealed_file("myapp", "development", STRIPE);
    broken_base64[20] = b'*';
    let mut inner_newline = sealed_file("myapp", "development", STRIPE);
    inner_newline.insert(30, b'\n');

    let version = load_bytes::<Credentials>(&version_two).unwrap_err();

    assert!(matches!(version, Error::UnsupportedVersion));
    assert_eq!(version.to_string(), "unsupported credseal file version");
    for contents in [
        b"[stripe]\nsecret_key = \"x\"\n".to_vec(),
        broken_base64,
        inner_newline,
        b"credseal-v1".to_vec(),
    ] {
        let error = load_bytes::<Credentials>(&contents).unwrap_err();
        assert!(matches!(error, Error::Malformed), "{contents:?}");
        assert_eq!(error.to_string(), "malformed credseal file");
    }
}

#[test]
fn invalid_toml_reports_the_line_but_not_the_text() {
    let file = sealed_file(
        "myapp",
        "development",
        b"region = \"eu\"\ntoken = \"sk_live_SECRET\n",
    );

    let error = load_bytes::<Credentials>(&file).unwrap_err();

    assert!(matches!(error, Error::InvalidToml { line: Some(2) }));
    assert_eq!(error.to_string(), "credentials are not valid TOML (line 2)");
    assert!(!format!("{error:?}").contains("SECRET"));
}

#[test]
fn decrypted_bytes_that_are_not_utf8_are_invalid_toml() {
    let error = load_bytes::<Credentials>(&sealed_file("myapp", "development", b"a = \"\xff\"\n"))
        .unwrap_err();

    assert!(matches!(error, Error::InvalidToml { line: None }));
    assert_eq!(error.to_string(), "credentials are not valid TOML");
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct Strict {
    #[serde(deserialize_with = "reject_with_secret")]
    token: String,
}

fn reject_with_secret<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    Err(serde::de::Error::custom(format!("bad token {value}")))
}

#[test]
fn a_custom_deserialize_error_never_reaches_the_message() {
    let file = sealed_file(
        "myapp",
        "development",
        b"# header\ntoken = \"sk_live_SECRET\"\n",
    );

    let error = load_bytes::<Strict>(&file).unwrap_err();

    assert!(matches!(error, Error::Deserialize { line: Some(2) }));
    assert_eq!(
        error.to_string(),
        "credentials do not match the expected type (line 2)"
    );
    assert!(!format!("{error:?}").contains("SECRET"));
    assert!(std::error::Error::source(&error).is_none());
}

#[test]
fn a_missing_field_is_a_type_mismatch() {
    let file = sealed_file("myapp", "development", b"[stripe]\nsecret_key = \"x\"\n");

    let error = load_bytes::<Credentials>(&file).unwrap_err();

    assert!(matches!(error, Error::Deserialize { .. }));
}

#[test]
fn a_missing_file_names_its_path() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("production.toml.enc");

    let error = CredentialsFile::new(&path, context("myapp", "production"))
        .load::<Credentials>(&key())
        .unwrap_err();

    assert!(
        matches!(&error, Error::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound)
    );
    assert_eq!(
        error.to_string(),
        format!(
            "cannot open {}: No such file or directory (os error 2)",
            path.display()
        )
    );
}

#[test]
fn a_directory_is_not_a_credentials_file() {
    let directory = TempDir::new().unwrap();

    let error = CredentialsFile::new(directory.path(), context("myapp", "development"))
        .load::<Credentials>(&key())
        .unwrap_err();

    assert!(matches!(error, Error::NotRegularFile { .. }));
}

#[cfg(unix)]
#[test]
fn a_symbolic_link_is_not_a_credentials_file() {
    let directory = TempDir::new().unwrap();
    let target = write(&directory, &sealed_file("myapp", "development", STRIPE));
    let link = directory.path().join("link.toml.enc");
    std::os::unix::fs::symlink(&target, &link).unwrap();

    let error = CredentialsFile::new(&link, context("myapp", "development"))
        .load::<Credentials>(&key())
        .unwrap_err();

    assert!(matches!(error, Error::NotRegularFile { .. }));
    assert_eq!(
        error.to_string(),
        format!("{} is not a regular file", link.display())
    );
}

#[test]
fn a_file_over_two_mebibytes_is_rejected_before_decryption() {
    let mut file = b"credseal-v1\n".to_vec();
    file.resize(2 * 1024 * 1024 + 1, b'A');

    let error = load_bytes::<Credentials>(&file).unwrap_err();

    assert!(matches!(
        error,
        Error::TooLarge {
            limit: 2_097_152,
            ..
        }
    ));
}

#[test]
fn a_key_must_be_base64_of_exactly_32_bytes() {
    for encoded in [
        "",
        "not a key",
        "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHg==",
        "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8gIQ==",
        "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8",
        "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh*=",
    ] {
        let error = MasterKey::from_base64(encoded).unwrap_err();
        assert!(matches!(error, Error::MalformedKey), "{encoded}");
        assert_eq!(
            error.to_string(),
            "the master key is not Base64 of exactly 32 bytes"
        );
    }
    let padded = MasterKey::from_base64(&format!("  {KEY_BASE64}\n")).unwrap();
    assert_eq!(padded.to_base64().as_str(), KEY_BASE64);
}

#[test]
fn a_key_never_shows_in_debug_output() {
    assert_eq!(format!("{:?}", key()), "MasterKey([REDACTED])");
}

#[test]
fn a_generated_key_round_trips_through_base64() {
    let generated = MasterKey::generate().unwrap();
    let encoded = generated.to_base64();

    assert_eq!(encoded.len(), 44);
    assert_eq!(
        MasterKey::from_base64(&encoded)
            .unwrap()
            .to_base64()
            .as_str(),
        encoded.as_str()
    );
    assert_ne!(
        encoded.as_str(),
        MasterKey::generate().unwrap().to_base64().as_str()
    );
}

#[test]
fn application_and_environment_names_are_lowercase_identifiers() {
    let longest = "a".repeat(64);
    assert_eq!(
        Context::new("my_app-2", &longest).unwrap().environment(),
        longest
    );

    for (application, environment, field) in [
        ("", "production", "application"),
        ("MyApp", "production", "application"),
        ("myapp", "../production", "environment"),
        ("myapp", "prod.uction", "environment"),
        ("myapp", &"a".repeat(65), "environment"),
    ] {
        let error = Context::new(application, environment).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("invalid {field} name: use 1 to 64 characters from a-z, 0-9, _ and -")
        );
    }
}

#[test]
fn a_duplicate_key_is_invalid_toml() {
    let file = sealed_file("myapp", "development", b"token = \"a\"\ntoken = \"b\"\n");

    let error = load_bytes::<serde::de::IgnoredAny>(&file).unwrap_err();

    assert_eq!(error.to_string(), "credentials are not valid TOML (line 2)");
}
