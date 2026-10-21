use std::collections::BTreeMap;

use loco_rs::{Error, Result};
use serde::Deserialize;

use crate::{
    settings::Settings,
    workspace::{valid_id, AdAccountChoice, Operation, WorkspaceStore},
};

#[derive(Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
}

#[derive(Deserialize)]
pub struct GoogleUser {
    pub sub: String,
    pub email: String,
    #[serde(default)]
    pub name: String,
    pub email_verified: bool,
}

pub async fn exchange(settings: &Settings, code: &str, verifier: &str) -> Result<Tokens> {
    let (client_id, client_secret) = settings.credentials().ok_or_else(unavailable)?;
    let response = settings
        .http
        .post(&settings.google.token_url)
        .form(&[
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("code", code),
            ("code_verifier", verifier),
            ("grant_type", "authorization_code"),
            ("redirect_uri", &settings.redirect_uri),
        ])
        .send()
        .await
        .map_err(|_| {
            tracing::warn!(
                operation = "token",
                failure = "transport",
                "Google call failed"
            );
            unavailable()
        })?;
    let status = response.status();
    if !status.is_success() {
        warn_google_error(response, "token", None, &[code, verifier, client_secret]).await?;
        return Err(unavailable());
    }
    let tokens: Tokens = response.json().await.map_err(|_| {
        tracing::warn!(
            operation = "token",
            http_status = status.as_u16(),
            failure = "invalid_response",
            "Google call failed"
        );
        unavailable()
    })?;
    if tokens.access_token.is_empty() {
        tracing::warn!(
            operation = "token",
            http_status = status.as_u16(),
            failure = "missing_access_token",
            "Google call failed"
        );
        return Err(unavailable());
    }
    Ok(tokens)
}

pub enum Refreshed {
    AccessToken(String),
    Revoked,
}

pub async fn revoke(settings: &Settings, token: &str) -> Result<()> {
    let response = settings
        .http
        .post(&settings.google.revoke_url)
        .form(&[("token", token)])
        .send()
        .await
        .map_err(|_| unavailable())?;
    if response.status().is_success() {
        return Ok(());
    }
    if response.status() == reqwest::StatusCode::BAD_REQUEST {
        let body = response
            .json::<serde_json::Value>()
            .await
            .map_err(|_| unavailable())?;
        if body["error"] == "invalid_token" {
            return Ok(());
        }
    }
    Err(unavailable())
}

pub async fn refresh(settings: &Settings, refresh_token: &str) -> Result<Refreshed> {
    let (client_id, client_secret) = settings.credentials().ok_or_else(unavailable)?;
    let response = settings
        .http
        .post(&settings.google.token_url)
        .form(&[
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await
        .map_err(|_| {
            tracing::warn!(
                operation = "token_refresh",
                failure = "transport",
                "Google call failed"
            );
            unavailable()
        })?;
    let status = response.status();
    if !status.is_success() {
        warn_google_error(
            response,
            "token_refresh",
            None,
            &[refresh_token, client_secret],
        )
        .await?;
        return if status == reqwest::StatusCode::BAD_REQUEST {
            Ok(Refreshed::Revoked)
        } else {
            Err(unavailable())
        };
    }
    let tokens: Tokens = response.json().await.map_err(|_| {
        tracing::warn!(
            operation = "token_refresh",
            http_status = status.as_u16(),
            failure = "invalid_response",
            "Google call failed"
        );
        unavailable()
    })?;
    if tokens.access_token.is_empty() {
        return Err(unavailable());
    }
    Ok(Refreshed::AccessToken(tokens.access_token))
}

pub async fn userinfo(settings: &Settings, access_token: &str) -> Result<GoogleUser> {
    let response = settings
        .http
        .get(&settings.google.userinfo_url)
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|_| {
            tracing::warn!(
                operation = "userinfo",
                failure = "transport",
                "Google call failed"
            );
            unavailable()
        })?;
    let status = response.status();
    if !status.is_success() {
        warn_google_error(response, "userinfo", None, &[access_token]).await?;
        return Err(unavailable());
    }
    let user: GoogleUser = response.json().await.map_err(|_| {
        tracing::warn!(
            operation = "userinfo",
            http_status = status.as_u16(),
            failure = "invalid_response",
            "Google call failed"
        );
        unavailable()
    })?;
    if user.sub.is_empty() || user.email.is_empty() || !user.email_verified {
        tracing::warn!(
            operation = "userinfo",
            http_status = status.as_u16(),
            failure = "unverified_identity",
            "Google call failed"
        );
        return Err(unavailable());
    }
    Ok(user)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Accessible {
    resource_names: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Search {
    #[serde(default)]
    results: Vec<SearchRow>,
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchRow {
    customer_client: GoogleAdAccount,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleAdAccount {
    id: String,
    #[serde(default)]
    descriptive_name: String,
    #[serde(default)]
    manager: bool,
}

pub async fn discover(settings: &Settings, access_token: &str) -> Result<Vec<AdAccountChoice>> {
    discover_scoped(settings, access_token, None).await
}

pub async fn discover_for_workspace(
    settings: &Settings,
    access_token: &str,
    store: &WorkspaceStore<'_>,
) -> Result<Vec<AdAccountChoice>> {
    discover_scoped(settings, access_token, Some(store)).await
}

async fn claim_operation(settings: &Settings, store: Option<&WorkspaceStore<'_>>) -> Result<()> {
    if let Some(store) = store {
        if !store
            .claim_google_operation(settings.google.daily_limit, Operation::Daily)
            .await?
        {
            return Err(Error::Message(
                "Google Ads quota or rate limit reached. Try again later.".into(),
            ));
        }
    }
    Ok(())
}

async fn discover_scoped(
    settings: &Settings,
    access_token: &str,
    store: Option<&WorkspaceStore<'_>>,
) -> Result<Vec<AdAccountChoice>> {
    claim_operation(settings, store).await?;
    let base = settings.google.ads_api_base.trim_end_matches('/');
    let started = std::time::Instant::now();
    let outcome = async {
        let response = settings
            .http
            .get(format!("{base}/customers:listAccessibleCustomers"))
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|_| {
                tracing::warn!(
                    operation = "customers:listAccessibleCustomers",
                    failure = "transport",
                    "Google call failed"
                );
                unavailable()
            })?;
        let status = response.status();
        if !status.is_success() {
            warn_google_error(
                response,
                "customers:listAccessibleCustomers",
                None,
                &[access_token],
            )
            .await?;
            return Err(unavailable());
        }
        let accessible: Accessible = response.json().await.map_err(|_| {
            tracing::warn!(
                operation = "customers:listAccessibleCustomers",
                http_status = status.as_u16(),
                failure = "invalid_response",
                "Google call failed"
            );
            unavailable()
        })?;
        Ok::<_, Error>(accessible)
    }
    .await;
    if let Some(store) = store {
        store
            .record_operation(
                crate::workspace::measurement::Operation::GoogleRequest,
                "",
                outcome.is_ok(),
                started.elapsed(),
                Default::default(),
            )
            .await;
    }
    let accessible = outcome?;
    let mut choices = BTreeMap::new();
    for resource in accessible.resource_names {
        let Some(id) = resource
            .strip_prefix("customers/")
            .filter(|id| valid_id(id))
        else {
            continue;
        };
        let Some(roots) = search(settings, access_token, id, None, "SELECT customer_client.id, customer_client.descriptive_name, customer_client.manager FROM customer_client WHERE customer_client.level = 0",store).await? else {
            continue;
        };
        let Some(root) = roots.into_iter().find(|root| root.id == id) else {
            continue;
        };
        if root.manager {
            let Some(rows) = search(settings, access_token, id, Some(id), "SELECT customer_client.id, customer_client.descriptive_name, customer_client.manager FROM customer_client WHERE customer_client.manager = FALSE AND customer_client.status = 'ENABLED'",store).await? else {
                continue;
            };
            for row in rows.into_iter().filter(|row| !row.manager) {
                if !valid_id(&row.id) {
                    return Err(unavailable());
                }
                choices.entry(row.id.clone()).or_insert(AdAccountChoice {
                    name: if row.descriptive_name.is_empty() {
                        row.id.clone()
                    } else {
                        row.descriptive_name
                    },
                    customer_id: row.id,
                    login_customer_id: Some(id.into()),
                });
            }
        } else {
            choices.insert(
                root.id.clone(),
                AdAccountChoice {
                    name: if root.descriptive_name.is_empty() {
                        root.id.clone()
                    } else {
                        root.descriptive_name
                    },
                    customer_id: root.id,
                    login_customer_id: None,
                },
            );
        }
    }
    Ok(choices.into_values().collect())
}

async fn search(
    settings: &Settings,
    access_token: &str,
    id: &str,
    manager: Option<&str>,
    query: &str,
    store: Option<&WorkspaceStore<'_>>,
) -> Result<Option<Vec<GoogleAdAccount>>> {
    let base = settings.google.ads_api_base.trim_end_matches('/');
    let mut rows = Vec::new();
    let mut page_token = String::new();
    let mut seen = std::collections::HashSet::new();
    loop {
        claim_operation(settings, store).await?;
        let mut body = serde_json::json!({ "query": query });
        if !page_token.is_empty() {
            body["pageToken"] = serde_json::json!(page_token);
        }
        let mut request = settings
            .http
            .post(format!("{base}/customers/{id}/googleAds:search"))
            .bearer_auth(access_token)
            .json(&body);
        if let Some(manager) = manager {
            request = request.header("login-customer-id", manager);
        }
        let started = std::time::Instant::now();
        let outcome = async {
            let response = request.send().await.map_err(|_| {
                tracing::warn!(
                    operation = "googleAds:search",
                    customer_id = id,
                    failure = "transport",
                    "Google call failed"
                );
                unavailable()
            })?;
            let status = response.status();
            if !status.is_success() {
                warn_google_error(response, "googleAds:search", Some(id), &[access_token]).await?;
                return if status == reqwest::StatusCode::UNAUTHORIZED {
                    Err(unavailable())
                } else {
                    Ok(None)
                };
            }
            let page: Search = response.json().await.map_err(|_| {
                tracing::warn!(
                    operation = "googleAds:search",
                    customer_id = id,
                    http_status = status.as_u16(),
                    failure = "invalid_response",
                    "Google call failed"
                );
                unavailable()
            })?;
            Ok::<_, Error>(Some((page, status)))
        }
        .await;
        if let Some(store) = store {
            store
                .record_operation(
                    crate::workspace::measurement::Operation::GoogleRequest,
                    "",
                    outcome.as_ref().is_ok_and(Option::is_some),
                    started.elapsed(),
                    Default::default(),
                )
                .await;
        }
        let Some((page, status)) = outcome? else {
            return Ok(None);
        };
        rows.extend(page.results.into_iter().map(|row| row.customer_client));
        let Some(next) = page.next_page_token.filter(|value| !value.is_empty()) else {
            break;
        };
        if !seen.insert(next.clone()) {
            tracing::warn!(
                operation = "googleAds:search",
                customer_id = id,
                http_status = status.as_u16(),
                failure = "repeated_page_token",
                "Google call failed"
            );
            return Err(unavailable());
        }
        page_token = next;
    }
    Ok(Some(rows))
}

async fn warn_google_error(
    response: reqwest::Response,
    operation: &str,
    customer_id: Option<&str>,
    secrets: &[&str],
) -> Result<()> {
    let status = response.status();
    let body = response.json::<serde_json::Value>().await;
    let transport_failure = body.as_ref().is_err_and(|error| !error.is_decode());
    let body = body.unwrap_or_default();
    let error = &body["error"];
    let code = error
        .as_str()
        .or_else(|| error["status"].as_str())
        .unwrap_or_default();
    let message = body["error_description"]
        .as_str()
        .or_else(|| error["message"].as_str())
        .unwrap_or_default();
    let ads_codes: Vec<_> = error["details"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|detail| detail["errors"].as_array())
        .flatten()
        .filter_map(|error| error["errorCode"].as_object())
        .flat_map(|code| code.values())
        .filter_map(serde_json::Value::as_str)
        .collect();
    tracing::warn!(
        operation,
        customer_id,
        http_status = status.as_u16(),
        google_error = safe_error_text(code, secrets),
        google_message = safe_error_text(message, secrets),
        google_ads_error = safe_error_text(&ads_codes.join(", "), secrets),
        "Google call failed"
    );
    if transport_failure {
        Err(unavailable())
    } else {
        Ok(())
    }
}

fn safe_error_text(value: &str, secrets: &[&str]) -> String {
    let mut value = value.to_owned();
    for secret in secrets.iter().filter(|secret| !secret.is_empty()) {
        value = value.replace(secret, "[REDACTED]");
    }
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(256)
        .collect()
}

fn unavailable() -> Error {
    Error::Message("Google could not complete this request. Please try again.".into())
}
