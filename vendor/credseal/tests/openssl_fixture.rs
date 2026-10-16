//! Compatibility with an independently encrypted credentials file.

use credseal::{Context, CredentialsFile, MasterKey, SecretString};
use serde::Deserialize;

#[derive(Deserialize)]
struct Credentials {
    stripe: Stripe,
}

#[derive(Deserialize)]
struct Stripe {
    secret_key: SecretString,
}

#[test]
fn an_application_reads_credentials_encrypted_by_openssl() {
    let key = MasterKey::from_base64("AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=").unwrap();
    let context = Context::new("myapp", "production").unwrap();
    let file = CredentialsFile::new(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/production.toml.enc"
        ),
        context,
    );

    let credentials: Credentials = file.load(&key).unwrap();

    assert_eq!(
        credentials.stripe.secret_key.expose_secret(),
        "openssl-fixture-token"
    );
}
