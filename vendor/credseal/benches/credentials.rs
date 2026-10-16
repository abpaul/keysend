//! Warm-cache microbenchmark of loading nine typed credentials: from the
//! process environment, and from a sealed file through
//! [`CredentialsFile::load`].
//!
//! The sealed timing covers open, metadata, read, authentication, decryption,
//! TOML parsing and deserialization into nine fields. Parsing the master key
//! and building the [`CredentialsFile`] happen once, outside the timing. Every
//! iteration checks all nine values and panics on any error or mismatch.
//!
//! `fixtures/credentials.toml` holds fake values only; `fixtures/generate.rb`
//! encrypts it with the public test key 0..31 into
//! `fixtures/production.toml.enc`.
//!
//! ```text
//! cargo bench --bench credentials -- [--rounds N] [--iterations N] [--profile env|sealed]
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};

use credseal::{Context, CredentialsFile, MasterKey};
use serde::Deserialize;

const KEY: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";
const SEALED_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/benches/fixtures/production.toml.enc"
);
const PLAINTEXT: &[u8] = include_bytes!("fixtures/credentials.toml");
const WARMUP: u64 = 1_000;
const PROFILE_FOR: Duration = Duration::from_secs(15);

const EXPECTED: [(&str, &str); 9] = [
    ("POSTDEPLOY_INGEST_KEY", "fake-postdeploy-ingest-key-0123456789abcdef0123456789abcdef"),
    ("STRIPE_SECRET_KEY", "sk_test_fake-0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"),
    ("STRIPE_WEBHOOK_SECRET", "fake-stripe-webhook-secret-0123456789abcdefghijklmnop"),
    ("OPENAI_API_KEY", "fake-openai-api-key-0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"),
    ("CLOUDFLARE_EMAIL_TOKEN", "fake-cloudflare-email-token-0123456789abcdefghijkl"),
    ("REPORT_EMAIL_RECIPIENT", "reports@example.com"),
    ("GOOGLE_CLIENT_ID", "000000000000-fakegoogleclientid0123456789abcdef.apps.googleusercontent.com"),
    ("GOOGLE_CLIENT_SECRET", "fake-google-client-secret-0123456789ab"),
    ("KEYSEND_MASTER_KEY", "ZmFrZS1rZXlzZW5kLW1hc3Rlci1rZXktbm90LXJlYWw="),
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Credentials {
    postdeploy_ingest_key: Option<String>,
    stripe_secret_key: Option<String>,
    stripe_webhook_secret: Option<String>,
    openai_api_key: Option<String>,
    cloudflare_email_token: Option<String>,
    report_email_recipient: Option<String>,
    google_client_id: Option<String>,
    google_client_secret: Option<String>,
    keysend_master_key: Option<String>,
}

impl Credentials {
    fn fields(&self) -> [&Option<String>; 9] {
        [
            &self.postdeploy_ingest_key,
            &self.stripe_secret_key,
            &self.stripe_webhook_secret,
            &self.openai_api_key,
            &self.cloudflare_email_token,
            &self.report_email_recipient,
            &self.google_client_id,
            &self.google_client_secret,
            &self.keysend_master_key,
        ]
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Case {
    Env,
    Sealed,
}

impl Case {
    fn name(self) -> &'static str {
        match self {
            Case::Env => "env",
            Case::Sealed => "sealed",
        }
    }
}

struct Bench {
    file: CredentialsFile,
    key: MasterKey,
    verified_fields: u64,
}

impl Bench {
    fn load(&self, case: Case) -> Credentials {
        match case {
            Case::Env => from_environment(),
            Case::Sealed => black_box(&self.file)
                .load(black_box(&self.key))
                .unwrap_or_else(|error| panic!("sealed load failed: {error}")),
        }
    }

    fn iteration(&mut self, case: Case) {
        let credentials = self.load(case);
        for (field, (name, expected)) in credentials.fields().into_iter().zip(EXPECTED) {
            if field.as_deref() != Some(expected) {
                panic!("{} returned a wrong {name}", case.name());
            }
        }
        self.verified_fields += 9;
        black_box(credentials);
    }

    fn run(&mut self, case: Case, iterations: u64) -> Duration {
        let start = Instant::now();
        for _ in 0..iterations {
            self.iteration(black_box(case));
        }
        start.elapsed()
    }
}

fn from_environment() -> Credentials {
    let read = |name: &str| std::env::var(black_box(name)).ok();
    Credentials {
        postdeploy_ingest_key: read("POSTDEPLOY_INGEST_KEY"),
        stripe_secret_key: read("STRIPE_SECRET_KEY"),
        stripe_webhook_secret: read("STRIPE_WEBHOOK_SECRET"),
        openai_api_key: read("OPENAI_API_KEY"),
        cloudflare_email_token: read("CLOUDFLARE_EMAIL_TOKEN"),
        report_email_recipient: read("REPORT_EMAIL_RECIPIENT"),
        google_client_id: read("GOOGLE_CLIENT_ID"),
        google_client_secret: read("GOOGLE_CLIENT_SECRET"),
        keysend_master_key: read("KEYSEND_MASTER_KEY"),
    }
}

struct Options {
    rounds: u64,
    iterations: u64,
    profile: Option<Case>,
}

fn options() -> Result<Options, String> {
    let mut options = Options {
        rounds: 5,
        iterations: 20_000,
        profile: None,
    };
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        let mut value = || {
            arguments
                .next()
                .ok_or_else(|| format!("{argument} needs a value"))
        };
        match argument.as_str() {
            "--bench" => {}
            "--rounds" => options.rounds = count(&argument, &value()?)?,
            "--iterations" => options.iterations = count(&argument, &value()?)?,
            "--profile" => {
                options.profile = Some(match value()?.as_str() {
                    "env" => Case::Env,
                    "sealed" => Case::Sealed,
                    other => return Err(format!("--profile takes env or sealed, not {other}")),
                })
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(options)
}

fn count(argument: &str, value: &str) -> Result<u64, String> {
    match value.parse() {
        Ok(count) if count > 0 => Ok(count),
        _ => Err(format!("{argument} takes a positive integer, not {value}")),
    }
}

fn median(sorted: &[f64]) -> f64 {
    let middle = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        (sorted[middle - 1] + sorted[middle]) / 2.0
    } else {
        sorted[middle]
    }
}

fn main() {
    let options = options().unwrap_or_else(|message| {
        eprintln!("credentials bench: {message}");
        std::process::exit(2);
    });

    for (name, value) in EXPECTED {
        std::env::set_var(name, value);
    }
    let file_bytes = std::fs::metadata(SEALED_PATH)
        .expect("sealed fixture is readable")
        .len();
    let encoded = (12 + PLAINTEXT.len() as u64 + 16).div_ceil(3) * 4;
    assert_eq!(
        file_bytes,
        "credseal-v1\n".len() as u64 + encoded + 1,
        "production.toml.enc does not match credentials.toml; run fixtures/generate.rb"
    );
    let env_bytes: usize = EXPECTED.iter().map(|(_, value)| value.len()).sum();

    let mut bench = Bench {
        file: CredentialsFile::new(
            SEALED_PATH,
            Context::new("keysend", "production").expect("valid context"),
        ),
        key: MasterKey::from_base64(KEY).expect("valid test key"),
        verified_fields: 0,
    };

    if let Some(case) = options.profile {
        let start = Instant::now();
        let mut iterations = 0;
        while start.elapsed() < PROFILE_FOR {
            bench.run(case, 1_000);
            iterations += 1_000;
        }
        println!(
            "profile {} iterations={iterations} verified_fields={} errors=0",
            case.name(),
            bench.verified_fields
        );
        return;
    }

    for case in [Case::Env, Case::Sealed] {
        bench.run(case, WARMUP);
    }
    let warmup_fields = bench.verified_fields;

    let mut results: [Vec<f64>; 2] = [Vec::new(), Vec::new()];
    for round in 0..options.rounds {
        let order = if round.is_multiple_of(2) {
            [Case::Env, Case::Sealed]
        } else {
            [Case::Sealed, Case::Env]
        };
        for case in order {
            let elapsed = bench.run(case, options.iterations);
            let per_op = elapsed.as_nanos() as f64 / options.iterations as f64;
            println!(
                "round {} {:<6} {per_op:>10.1} ns/op",
                round + 1,
                case.name()
            );
            results[case as usize].push(per_op);
        }
    }

    println!(
        "iterations/round={} rounds={} fields/op=9 verified_fields={} (plus {warmup_fields} in warmup) errors=0",
        options.iterations,
        options.rounds,
        bench.verified_fields - warmup_fields
    );
    for (case, input) in [
        (Case::Env, format!("env_value_bytes={env_bytes}")),
        (
            Case::Sealed,
            format!(
                "plaintext_bytes={} file_bytes={file_bytes}",
                PLAINTEXT.len()
            ),
        ),
    ] {
        let mut runs = results[case as usize].clone();
        runs.sort_by(f64::total_cmp);
        println!(
            "{:<6} median {:>10.1} ns/op  range {:.1}..{:.1}  {input}",
            case.name(),
            median(&runs),
            runs[0],
            runs[runs.len() - 1]
        );
    }
}
