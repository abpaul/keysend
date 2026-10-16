//! Loads typed credentials and reads their fields.
//!
//! ```sh
//! export CREDSEAL_MASTER_KEY=...   # the development key
//! cargo run --example typed
//! ```

use credseal::{Context, CredentialsFile, MasterKey, SecretString};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Credentials {
    stripe: Stripe,
    database: Database,
}

#[derive(Debug, Deserialize)]
struct Stripe {
    publishable_key: String,
    secret_key: SecretString,
}

#[derive(Debug, Deserialize)]
struct Database {
    url: SecretString,
}

fn main() -> Result<(), credseal::Error> {
    let key = MasterKey::from_env()?;
    let context = Context::new("myapp", "development")?;
    let file = CredentialsFile::new("config/credentials/development.toml.enc", context);
    let credentials: Credentials = file.load(&key)?;

    println!(
        "Stripe publishable key: {}",
        credentials.stripe.publishable_key
    );
    println!("Loaded: {credentials:?}");
    connect(credentials.database.url.expose_secret());
    charge(credentials.stripe.secret_key.expose_secret());
    Ok(())
}

fn connect(url: &str) {
    println!("Database URL has {} characters", url.len());
}

fn charge(secret_key: &str) {
    println!("Stripe secret key has {} characters", secret_key.len());
}
