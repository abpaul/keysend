use std::{env, sync::Arc, time::Duration};

use loco_rs::{app::AppContext, environment::Environment, Error, Result};
use serde::Deserialize;

use crate::crypto::MasterKey;

mod credentials;

pub use credentials::{Credentials, Scope, Status};

#[derive(Deserialize)]
#[serde(default)]
pub struct GoogleSettings {
    pub revoke_url: String,
    pub authorization_url: String,
    pub token_url: String,
    pub userinfo_url: String,
    pub ads_api_base: String,
    pub daily_limit: u32,
}

impl Default for GoogleSettings {
    fn default() -> Self {
        Self {
            revoke_url: "https://oauth2.googleapis.com/revoke".into(),
            authorization_url: "https://accounts.google.com/o/oauth2/v2/auth".into(),
            token_url: "https://oauth2.googleapis.com/token".into(),
            userinfo_url: "https://openidconnect.googleapis.com/v1/userinfo".into(),
            ads_api_base: "https://googleads.googleapis.com/v25".into(),
            daily_limit: 15_000,
        }
    }
}

#[derive(Deserialize)]
#[serde(default)]
pub struct ServiceSettings {
    pub ai_url: String,
    pub ai_model: String,
    pub email_url: Option<String>,
    pub test_now: Option<chrono::DateTime<chrono::Utc>>,
}

impl Default for ServiceSettings {
    fn default() -> Self {
        Self {
            ai_url: "https://api.openai.com/v1/responses".into(),
            ai_model: "gpt-6.1-sol".into(),
            test_now: None,
            email_url: nonempty_env("CLOUDFLARE_ACCOUNT_ID").map(|id| {
                format!("https://api.cloudflare.com/client/v4/accounts/{id}/email/sending/send")
            }),
        }
    }
}

#[derive(Deserialize)]
#[serde(default)]
pub struct MonitoringSettings {
    pub first_update_wait_seconds: u64,
}

impl Default for MonitoringSettings {
    fn default() -> Self {
        Self {
            first_update_wait_seconds: 20,
        }
    }
}

pub struct Settings {
    pub postdeploy: PostDeploySettings,
    pub postdeploy_key: Option<String>,
    pub billing: BillingSettings,
    pub stripe_key: Option<String>,
    pub stripe_live: bool,
    pub stripe_webhook: Option<String>,
    pub services: ServiceSettings,
    pub monitoring: MonitoringSettings,
    pub openai_key: Option<String>,
    pub email_key: Option<String>,
    pub email_recipient: Option<String>,
    pub google: GoogleSettings,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub master_key: Option<MasterKey>,
    pub origin: String,
    pub redirect_uri: String,
    pub secure: bool,
    pub http: reqwest::Client,
}

impl Settings {
    pub fn now(&self) -> chrono::DateTime<chrono::Utc> {
        self.services.test_now.unwrap_or_else(wall_clock)
    }
    pub fn payments_ready(&self) -> bool {
        self.stripe_key.is_some() && self.stripe_webhook.is_some()
    }

    pub fn load(ctx: &AppContext) -> Result<Self> {
        #[derive(Default, Deserialize)]
        struct ApplicationSettings {
            #[serde(default)]
            postdeploy: PostDeploySettings,
            #[serde(default)]
            billing: BillingSettings,
            #[serde(default)]
            google: GoogleSettings,
            #[serde(default)]
            services: ServiceSettings,
            #[serde(default)]
            monitoring: MonitoringSettings,
        }
        let application: ApplicationSettings = ctx
            .config
            .settings
            .clone()
            .map(serde_json::from_value)
            .transpose()?
            .unwrap_or_default();
        if application.services.test_now.is_some() && ctx.environment != Environment::Test {
            return Err(Error::Message(
                "A clock override is allowed only in tests".into(),
            ));
        }
        let credentials = ctx
            .shared_store
            .get::<Arc<Credentials>>()
            .ok_or_else(|| Error::Message("Credentials are not loaded".into()))?;
        let master_key = credentials.master_key(&ctx.environment)?;
        let mut host = reqwest::Url::parse(&ctx.config.server.host)
            .map_err(|_| Error::Message("Invalid server host".into()))?;
        if !matches!(host.scheme(), "http" | "https") || host.host_str().is_none() {
            return Err(Error::Message("Invalid server host".into()));
        }
        if host.port().is_none()
            && host.scheme() == "http"
            && ctx.environment != Environment::Production
        {
            host.set_port(Some(ctx.config.server.port as u16))
                .map_err(|_| Error::Message("Invalid server port".into()))?;
        }
        let origin = host.origin().ascii_serialization();
        let stripe_live = credentials.stripe_live()?;
        Ok(Self {
            postdeploy: application.postdeploy,
            postdeploy_key: credentials.postdeploy_ingest_key.clone(),
            billing: application.billing,
            stripe_key: credentials.stripe_secret_key.clone(),
            stripe_live,
            stripe_webhook: credentials.stripe_webhook_secret.clone(),
            google: application.google,
            services: application.services,
            monitoring: application.monitoring,
            openai_key: credentials.openai_api_key.clone(),
            email_key: credentials.cloudflare_email_token.clone(),
            email_recipient: credentials.report_email_recipient.clone(),
            client_id: credentials.google_client_id.clone(),
            client_secret: credentials.google_client_secret.clone(),
            master_key,
            secure: host.scheme() == "https",
            redirect_uri: format!("{origin}/auth/google/callback"),
            origin,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| Error::Message("Cannot create Google HTTP client".into()))?,
        })
    }

    pub fn credentials(&self) -> Option<(&str, &str)> {
        Some((self.client_id.as_deref()?, self.client_secret.as_deref()?))
    }
}

#[derive(Deserialize)]
#[serde(default)]
pub struct PostDeploySettings {
    pub url: String,
}

impl Default for PostDeploySettings {
    fn default() -> Self {
        Self {
            url: "https://ingest.postdeploy.dev/e/errors".into(),
        }
    }
}

#[derive(Deserialize)]
#[serde(default)]
pub struct BillingSettings {
    pub api_base: String,
}

impl Default for BillingSettings {
    fn default() -> Self {
        Self {
            api_base: "https://api.stripe.com/v1".into(),
        }
    }
}

#[allow(clippy::disallowed_methods)]
pub fn wall_clock() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now()
}

pub fn transport_kind(error: &reqwest::Error) -> &'static str {
    if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connect"
    } else if error.is_decode() {
        "decode"
    } else {
        "other"
    }
}

fn nonempty_env(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.is_empty())
}
