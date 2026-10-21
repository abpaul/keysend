use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

use loco_rs::{config::Config, environment::Environment, Error, Result};
use serde::{Deserialize, Serialize};

use crate::crypto::MasterKey;

const UNLOCK_KEY: &str = "CREDSEAL_MASTER_KEY";
const RCLONE_CONFIG: &str = "RCLONE_CONFIG_";
const BACKUP_REMOTE: &str = "keysend_r2:";

const CREDENTIAL_ENV_NAMES: [&str; 21] = [
    "POSTDEPLOY_INGEST_KEY",
    "STRIPE_SECRET_KEY",
    "STRIPE_WEBHOOK_SECRET",
    "OPENAI_API_KEY",
    "CLOUDFLARE_EMAIL_TOKEN",
    "REPORT_EMAIL_RECIPIENT",
    "ADMIN_EMAILS",
    "GOOGLE_CLIENT_ID",
    "GOOGLE_CLIENT_SECRET",
    "KEYSEND_MASTER_KEY",
    "KAMAL_REGISTRY_PASSWORD",
    "CLOUDFLARE_CACHE_TOKEN",
    "R2_ACCESS_KEY_ID",
    "R2_SECRET_ACCESS_KEY",
    "R2_ENDPOINT",
    "CLOUDFLARE_ORIGIN_CERTIFICATE",
    "CLOUDFLARE_ORIGIN_KEY",
    "CI_SSH_KEY",
    "CI_KNOWN_HOSTS",
    "AGE_RECIPIENT",
    "KEYSEND_CREDENTIALS_FILE",
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credentials {
    pub(super) postdeploy_ingest_key: Option<String>,
    pub(super) stripe_secret_key: Option<String>,
    pub(super) stripe_webhook_secret: Option<String>,
    pub(super) openai_api_key: Option<String>,
    pub(super) cloudflare_email_token: Option<String>,
    pub(super) admin_emails: Option<String>,
    pub(super) report_email_recipient: Option<String>,
    pub(super) google_client_id: Option<String>,
    pub(super) google_client_secret: Option<String>,
    pub(super) keysend_master_key: Option<String>,
    kamal_registry_password: Option<String>,
    cloudflare_cache_token: Option<String>,
    r2_access_key_id: Option<String>,
    r2_secret_access_key: Option<String>,
    r2_endpoint: Option<String>,
    cloudflare_origin_certificate: Option<String>,
    cloudflare_origin_key: Option<String>,
    ci_ssh_key: Option<String>,
    ci_known_hosts: Option<String>,
    age_recipient: Option<String>,
}

/// The tools that run with a part of the sealed credentials.
#[derive(Clone, Copy)]
pub enum Scope {
    /// The release and Kamal: the unlock key for the container, the registry,
    /// cache, origin TLS and CI SSH values, and the Stripe and error reporting
    /// values that the release checks.
    Deploy,
    /// The database backup: the age recipient and the `keysend_r2` rclone remote.
    Backup,
}

/// What a sealed file can do, without a value from it.
#[derive(Serialize)]
pub struct Status {
    pub payments_ready: bool,
    pub postdeploy_ready: bool,
    pub deploy_ready: bool,
    pub backup_ready: bool,
}

impl Credentials {
    /// Reads the sealed file when `CREDSEAL_MASTER_KEY` is set, even to an
    /// empty value, and the process environment otherwise. A sealed file is
    /// the only source: no field falls back to the environment.
    pub fn resolve(environment: &Environment, config: &Config) -> Result<Self> {
        if env::var_os(UNLOCK_KEY).is_some() {
            #[derive(Default, Deserialize)]
            struct Location {
                credentials_directory: Option<PathBuf>,
            }
            let location: Location = config
                .settings
                .clone()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default();
            let directory = location
                .credentials_directory
                .unwrap_or_else(|| PathBuf::from("config/credentials"));
            Self::open(environment, &directory)
        } else {
            Self::from_environment().validated(environment)
        }
    }

    /// Reads `{directory}/{environment}.toml.enc` with the key in
    /// `CREDSEAL_MASTER_KEY` and checks it as the application does at start.
    pub fn open(environment: &Environment, directory: &Path) -> Result<Self> {
        let path = Self::path(environment, directory);
        let failed = |error: credseal::Error| {
            Error::Message(format!(
                "Cannot load the credentials in {}: {error}",
                path.display()
            ))
        };
        let key = credseal::MasterKey::from_env().map_err(failed)?;
        let context =
            credseal::Context::new("keysend", &environment.to_string()).map_err(failed)?;
        credseal::CredentialsFile::new(&path, context)
            .load::<Self>(&key)
            .map_err(failed)?
            .validated(environment)
    }

    pub fn path(environment: &Environment, directory: &Path) -> PathBuf {
        directory.join(format!("{environment}.toml.enc"))
    }

    pub fn status(&self) -> Status {
        Status {
            payments_ready: self.stripe_secret_key.is_some()
                && self.stripe_webhook_secret.is_some(),
            postdeploy_ready: self.postdeploy_ingest_key.is_some(),
            deploy_ready: self.variables(Scope::Deploy).is_ok(),
            backup_ready: self.variables(Scope::Backup).is_ok(),
        }
    }

    /// Gives `command` the variables of `scope` and removes every other
    /// credential variable and rclone remote that it would inherit. The
    /// backup also loses the unlock key and needs `RCLONE_REMOTE` to name the
    /// `keysend_r2` remote.
    pub fn prepare(&self, scope: Scope, file: &Path, command: &mut Command) -> Result<()> {
        let variables = self.variables(scope)?;
        if matches!(scope, Scope::Backup) {
            let remote = env::var("RCLONE_REMOTE").unwrap_or_default();
            if remote
                .strip_prefix(BACKUP_REMOTE)
                .is_none_or(|bucket| bucket.is_empty())
            {
                return Err(Error::Message(format!(
                    "RCLONE_REMOTE must be {BACKUP_REMOTE}BUCKET for the sealed backup"
                )));
            }
            command.env_remove(UNLOCK_KEY);
        }
        for name in CREDENTIAL_ENV_NAMES {
            command.env_remove(name);
        }
        for (name, _) in env::vars_os() {
            if name.to_string_lossy().starts_with(RCLONE_CONFIG) {
                command.env_remove(name);
            }
        }
        if matches!(scope, Scope::Deploy) {
            command.env("KEYSEND_CREDENTIALS_FILE", file);
        }
        command.envs(variables);
        Ok(())
    }

    fn variables(&self, scope: Scope) -> Result<Vec<(&'static str, String)>> {
        let value = |field: &str, value: &Option<String>, required: bool| match value {
            Some(value) if value.contains('\0') => {
                Err(Error::Message(format!("{field} has a NUL byte")))
            }
            None if required => Err(Error::Message(format!(
                "The credentials have no {field}, which this scope needs"
            ))),
            value => Ok(value.clone()),
        };
        let fields = match scope {
            Scope::Deploy => vec![
                (
                    "KAMAL_REGISTRY_PASSWORD",
                    value(
                        "kamal_registry_password",
                        &self.kamal_registry_password,
                        true,
                    )?,
                ),
                (
                    "CLOUDFLARE_CACHE_TOKEN",
                    value("cloudflare_cache_token", &self.cloudflare_cache_token, true)?,
                ),
                (
                    "CLOUDFLARE_ORIGIN_CERTIFICATE",
                    value(
                        "cloudflare_origin_certificate",
                        &self.cloudflare_origin_certificate,
                        true,
                    )?,
                ),
                (
                    "CLOUDFLARE_ORIGIN_KEY",
                    value("cloudflare_origin_key", &self.cloudflare_origin_key, true)?,
                ),
                ("CI_SSH_KEY", value("ci_ssh_key", &self.ci_ssh_key, true)?),
                (
                    "CI_KNOWN_HOSTS",
                    value("ci_known_hosts", &self.ci_known_hosts, true)?,
                ),
                (
                    "STRIPE_SECRET_KEY",
                    value("stripe_secret_key", &self.stripe_secret_key, false)?,
                ),
                (
                    "STRIPE_WEBHOOK_SECRET",
                    value("stripe_webhook_secret", &self.stripe_webhook_secret, false)?,
                ),
                (
                    "POSTDEPLOY_INGEST_KEY",
                    value("postdeploy_ingest_key", &self.postdeploy_ingest_key, false)?,
                ),
            ],
            Scope::Backup => vec![
                (
                    "AGE_RECIPIENT",
                    value("age_recipient", &self.age_recipient, true)?,
                ),
                ("RCLONE_CONFIG_KEYSEND_R2_TYPE", Some("s3".into())),
                (
                    "RCLONE_CONFIG_KEYSEND_R2_PROVIDER",
                    Some("Cloudflare".into()),
                ),
                ("RCLONE_CONFIG_KEYSEND_R2_REGION", Some("auto".into())),
                (
                    "RCLONE_CONFIG_KEYSEND_R2_ACCESS_KEY_ID",
                    value("r2_access_key_id", &self.r2_access_key_id, true)?,
                ),
                (
                    "RCLONE_CONFIG_KEYSEND_R2_SECRET_ACCESS_KEY",
                    value("r2_secret_access_key", &self.r2_secret_access_key, true)?,
                ),
                (
                    "RCLONE_CONFIG_KEYSEND_R2_ENDPOINT",
                    value("r2_endpoint", &self.r2_endpoint, true)?,
                ),
                (
                    "RCLONE_CONFIG_KEYSEND_R2_NO_CHECK_BUCKET",
                    Some("true".into()),
                ),
                ("RCLONE_CONFIG_KEYSEND_R2_ENV_AUTH", Some("false".into())),
            ],
        };
        Ok(fields
            .into_iter()
            .filter_map(|(name, value)| Some((name, value?)))
            .collect())
    }

    pub(super) fn master_key(&self, environment: &Environment) -> Result<Option<MasterKey>> {
        let key = self
            .keysend_master_key
            .as_deref()
            .map(MasterKey::from_base64)
            .transpose()?;
        if *environment == Environment::Production && key.is_none() {
            return Err(Error::Message(
                "KEYSEND_MASTER_KEY is required in production".into(),
            ));
        }
        Ok(key)
    }

    pub(super) fn stripe_live(&self) -> Result<bool> {
        match self
            .stripe_secret_key
            .as_deref()
            .map(crate::billing::key_mode)
        {
            Some(Some(live)) => Ok(live),
            Some(None) => Err(Error::Message(
                "STRIPE_SECRET_KEY must be a secret or restricted Stripe key".into(),
            )),
            None => Ok(false),
        }
    }

    fn validated(self, environment: &Environment) -> Result<Self> {
        let credentials = self.without_empty_values();
        credentials.master_key(environment)?;
        credentials.stripe_live()?;
        Ok(credentials)
    }

    fn from_environment() -> Self {
        let read = |name| env::var(name).ok();
        Self {
            postdeploy_ingest_key: read("POSTDEPLOY_INGEST_KEY"),
            stripe_secret_key: read("STRIPE_SECRET_KEY"),
            stripe_webhook_secret: read("STRIPE_WEBHOOK_SECRET"),
            openai_api_key: read("OPENAI_API_KEY"),
            cloudflare_email_token: read("CLOUDFLARE_EMAIL_TOKEN"),
            admin_emails: read("ADMIN_EMAILS"),
            report_email_recipient: read("REPORT_EMAIL_RECIPIENT"),
            google_client_id: read("GOOGLE_CLIENT_ID"),
            google_client_secret: read("GOOGLE_CLIENT_SECRET"),
            keysend_master_key: read("KEYSEND_MASTER_KEY"),
            kamal_registry_password: None,
            cloudflare_cache_token: None,
            r2_access_key_id: None,
            r2_secret_access_key: None,
            r2_endpoint: None,
            cloudflare_origin_certificate: None,
            cloudflare_origin_key: None,
            ci_ssh_key: None,
            ci_known_hosts: None,
            age_recipient: None,
        }
    }

    fn without_empty_values(mut self) -> Self {
        for value in [
            &mut self.postdeploy_ingest_key,
            &mut self.stripe_secret_key,
            &mut self.stripe_webhook_secret,
            &mut self.openai_api_key,
            &mut self.cloudflare_email_token,
            &mut self.report_email_recipient,
            &mut self.admin_emails,
            &mut self.google_client_id,
            &mut self.google_client_secret,
            &mut self.keysend_master_key,
            &mut self.kamal_registry_password,
            &mut self.cloudflare_cache_token,
            &mut self.r2_access_key_id,
            &mut self.r2_secret_access_key,
            &mut self.r2_endpoint,
            &mut self.cloudflare_origin_certificate,
            &mut self.cloudflare_origin_key,
            &mut self.ci_ssh_key,
            &mut self.ci_known_hosts,
            &mut self.age_recipient,
        ] {
            if value.as_deref() == Some("") {
                *value = None;
            }
        }
        self
    }
}
