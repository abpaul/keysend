//! The changes that Optimize sends to Google Ads, over REST, for one Ad Account of one
//! Workspace. Only two services are called: `campaignBudgets:mutate` for a budget amount and
//! `campaigns:mutate` for a Campaign status, each with one operation, an exact update mask,
//! `partialFailure` off and `MUTABLE_RESOURCE` in the answer. Other kinds are refused here,
//! whatever the settings say. The resource names are built from the Ad Account that
//! `Client::connect` resolved within the Workspace, never from text of a Proposal.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{resource_id, text, Client, Failure};
use crate::{
    optimize::{
        ads::{Ads, AdsError, Applied},
        policy::Live,
        Action,
    },
    workspace::Operation,
};

impl From<Failure> for AdsError {
    fn from(failure: Failure) -> Self {
        match failure {
            Failure::Reconnect | Failure::Disabled => Self::Access,
            Failure::Quota => Self::Quota,
            Failure::DailyLimit | Failure::Unavailable => Self::Unavailable,
            Failure::Unsupported => Self::Refused {
                code: "UNSUPPORTED".into(),
            },
        }
    }
}

/// The REST name of the change and its body, or `None` for a kind this client never sends.
fn operation(customer: &str, action: &Action) -> Option<(&'static str, Value)> {
    Some(match action {
        Action::AdjustCampaignBudget {
            budget_id,
            to_micros,
            ..
        } => (
            "campaignBudgets",
            json!({"update": {"resourceName": format!("customers/{customer}/campaignBudgets/{budget_id}"), "amountMicros": to_micros.to_string()}, "updateMask": "amountMicros"}),
        ),
        Action::PauseCampaign { campaign_id } => (
            "campaigns",
            json!({"update": {"resourceName": format!("customers/{customer}/campaigns/{campaign_id}"), "status": "PAUSED"}, "updateMask": "status"}),
        ),
        _ => return None,
    })
}

fn codes(body: &Value) -> Vec<String> {
    body["error"]["details"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(
            body.pointer("/partialFailureError/details")
                .and_then(Value::as_array)
                .into_iter()
                .flatten(),
        )
        .flat_map(|detail| detail["errors"].as_array().into_iter().flatten())
        .filter_map(|error| error["errorCode"].as_object())
        .flat_map(|code| code.values())
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

fn answer(
    status: reqwest::StatusCode,
    body: &Value,
    request_id: Option<String>,
    validate_only: bool,
    resource: &str,
) -> Result<Applied, AdsError> {
    let codes = codes(body);
    let has = |code: &str| codes.iter().any(|found| found == code);
    if status.is_success() {
        if !body.is_object()
            || body.get("error").is_some()
            || body.get("partialFailureError").is_some()
        {
            return Err(AdsError::Unknown);
        }
        if !codes.is_empty() {
            return Err(AdsError::Unknown);
        }
        if !validate_only
            && !body["results"].as_array().is_some_and(|results| {
                results.len() == 1 && results[0]["resourceName"].as_str() == Some(resource)
            })
        {
            return Err(AdsError::Unknown);
        }
        return Ok(Applied { request_id });
    }
    Err(
        if status == 429 || has("RESOURCE_EXHAUSTED") || has("RESOURCE_TEMPORARILY_EXHAUSTED") {
            AdsError::Quota
        } else if status == 401
            || status == 403
            || has("OAUTH_TOKEN_REVOKED")
            || has("USER_PERMISSION_DENIED")
            || has("CUSTOMER_NOT_ENABLED")
        {
            AdsError::Access
        } else if status.is_server_error() || status == 408 || status.as_u16() == 499 {
            AdsError::Unknown
        } else {
            AdsError::Refused {
                code: codes
                    .first()
                    .cloned()
                    .unwrap_or_else(|| format!("HTTP_{}", status.as_u16())),
            }
        },
    )
}

#[async_trait]
impl Ads for Client<'_, '_> {
    async fn campaign(&self, campaign_id: i64) -> Result<Option<Live>, AdsError> {
        let rows = self
            .run(
                &format!("SELECT campaign.id, campaign.status, campaign.bidding_strategy_type, campaign_budget.id, campaign_budget.amount_micros, campaign_budget.period, campaign_budget.explicitly_shared, campaign_budget.reference_count FROM campaign WHERE campaign.id = {campaign_id}"),
                Operation::Later,
            )
            .await?;
        if rows
            .first()
            .is_some_and(|row| text(row, "/campaignBudget/period") != "DAILY")
        {
            return Err(AdsError::Refused {
                code: "KEYSEND_UNSUPPORTED_BUDGET_PERIOD".into(),
            });
        }
        Ok(rows.first().and_then(|row| {
            Some(Live {
                campaign_id: resource_id(row, "/campaign/id")?,
                status: text(row, "/campaign/status"),
                budget_id: resource_id(row, "/campaignBudget/id"),
                budget_micros: resource_id(row, "/campaignBudget/amountMicros"),
                budget_shared: row
                    .pointer("/campaignBudget/explicitlyShared")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
                budget_campaigns: resource_id(row, "/campaignBudget/referenceCount").unwrap_or(0),
                bidding_strategy: text(row, "/campaign/biddingStrategyType"),
            })
        }))
    }

    async fn apply(&self, action: &Action, validate_only: bool) -> Result<Applied, AdsError> {
        let policy = self
            .store
            .optimization_policy(&self.ad.customer_id)
            .await
            .map_err(|_| AdsError::Unavailable)?;
        if !crate::optimize::execute::live(self.settings, action)
            || matches!(
                policy.policy.mode,
                crate::optimize::Mode::Investigate | crate::optimize::Mode::Shadow
            )
            || self
                .store
                .optimization_stopped()
                .await
                .map_err(|_| AdsError::Unavailable)?
            || !self
                .store
                .monitoring_access(self.settings.now().timestamp())
                .await
                .map_err(|_| AdsError::Unavailable)?
        {
            return Err(AdsError::Unavailable);
        }
        let customer = &self.ad.customer_id;
        let (service, operation) = operation(customer, action).ok_or(AdsError::Refused {
            code: "UNSUPPORTED".into(),
        })?;
        let resource = operation["update"]["resourceName"]
            .as_str()
            .ok_or(AdsError::Unavailable)?
            .to_owned();
        let claimed = self
            .store
            .claim_google_operation(self.settings.google.daily_limit, Operation::Later)
            .await
            .map_err(|_| AdsError::Unavailable)?;
        if !claimed {
            return Err(AdsError::Unavailable);
        }
        let mut request = self
            .settings
            .http
            .post(format!(
                "{}/customers/{customer}/{service}:mutate",
                self.settings.google.ads_api_base.trim_end_matches('/'),
            ))
            .bearer_auth(&self.access)
            .json(&json!({"operations": [operation], "partialFailure": false, "validateOnly": validate_only, "responseContentType": "MUTABLE_RESOURCE"}));
        if let Some(manager) = &self.ad.login_customer_id {
            request = request.header("login-customer-id", manager);
        }
        let response = match request.send().await {
            Ok(response) => response,
            // A connection that never opened sent nothing; any other transport failure may
            // have reached Google Ads.
            Err(error) if error.is_connect() => return Err(AdsError::Unavailable),
            Err(error) => {
                tracing::warn!(
                    kind = crate::settings::transport_kind(&error),
                    service,
                    "Google Ads change has no answer"
                );
                return Err(AdsError::Unknown);
            }
        };
        let status = response.status();
        let request_id = response
            .headers()
            .get("request-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body: Value = response.json().await.unwrap_or(Value::Null);
        let codes = codes(&body);
        tracing::info!(
            service,
            validate_only,
            http_status = status.as_u16(),
            ?request_id,
            ?codes,
            "Google Ads change answered"
        );
        answer(status, &body, request_id, validate_only, &resource)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_shadow_mutation_adapter_has_no_write_capability_even_when_live_is_on() {
        use crate::{
            optimize::{
                policy::{Policy, Standing},
                Mode,
            },
            workspace::{Workspace, WorkspaceStore},
        };
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        let received = Arc::new(AtomicUsize::new(0));
        let counter = received.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = axum::Router::new().fallback(move || {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                axum::http::StatusCode::SERVICE_UNAVAILABLE
            }
        });
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut settings = crate::monitoring::eval::settings("http://127.0.0.1:9");
        settings.optimize.live = true;
        settings.optimize.max_mode = Mode::Autopilot;
        settings.google.ads_api_base = format!("http://{address}");
        let now = settings.now().timestamp();
        let db = crate::workspace::shadow_adapter_database(now)
            .await
            .unwrap();
        let workspace = Workspace::from_identity("w".into());
        let store = WorkspaceStore::new(&db, &workspace);
        assert!(store.monitoring_access(now).await.unwrap());
        store
            .save_optimization_policy(
                "1234567890",
                &Policy {
                    mode: Mode::Shadow,
                    ..Default::default()
                },
                Standing::Active,
                "shadow_consent",
                now,
            )
            .await
            .unwrap();
        let client = Client {
            settings: &settings,
            store: &store,
            ad: store.ad_accounts().await.unwrap().remove(0),
            access: "unused".into(),
        };
        for validate in [true, false] {
            assert_eq!(
                client
                    .apply(&Action::PauseCampaign { campaign_id: 9 }, validate)
                    .await,
                Err(AdsError::Unavailable)
            );
        }
        assert_eq!(received.load(Ordering::SeqCst), 0);
        store
            .save_optimization_policy(
                "1234567890",
                &Policy {
                    mode: Mode::Recommend,
                    ..Default::default()
                },
                Standing::Active,
                "",
                now,
            )
            .await
            .unwrap();
        assert!(client
            .apply(&Action::PauseCampaign { campaign_id: 9 }, true)
            .await
            .is_err());
        assert_eq!(
            received.load(Ordering::SeqCst),
            1,
            "the recording server receives an allowed validation request"
        );
        server.abort();
    }

    #[test]
    fn a_change_names_only_resources_of_its_ad_account_and_masks_only_its_field() {
        let (service, body) = operation(
            "4829153307",
            &Action::AdjustCampaignBudget {
                campaign_id: 1,
                budget_id: 77,
                from_micros: 20_000_000,
                to_micros: 16_000_000,
            },
        )
        .unwrap();
        assert_eq!(service, "campaignBudgets");
        assert_eq!(
            body,
            json!({"update":{"resourceName":"customers/4829153307/campaignBudgets/77","amountMicros":"16000000"},"updateMask":"amountMicros"})
        );
        let (service, body) =
            operation("4829153307", &Action::PauseCampaign { campaign_id: 9 }).unwrap();
        assert_eq!(service, "campaigns");
        assert_eq!(body["updateMask"], "status");
        assert_eq!(body["update"]["status"], "PAUSED");
        assert!(operation(
            "4829153307",
            &Action::AddNegativeKeyword {
                campaign_id: 1,
                text: "free".into(),
                match_type: crate::optimize::MatchType::Exact
            }
        )
        .is_none());
    }

    #[test]
    fn a_campaign_cannot_be_enabled_by_the_mutation_adapter() {
        assert!(operation("4829153307", &Action::EnableCampaign { campaign_id: 9 }).is_none());
    }

    #[test]
    fn a_malformed_mutation_answer_never_confirms_a_change() {
        use reqwest::StatusCode;
        let resource = "customers/4829153307/campaigns/9";
        for body in [
            Value::Null,
            json!({}),
            json!({"results": []}),
            json!({"results":[{"resourceName":"customers/4829153307/campaigns/8"}]}),
            json!({"partialFailureError":{"code":13}}),
        ] {
            assert_eq!(
                answer(StatusCode::OK, &body, None, false, resource),
                Err(AdsError::Unknown)
            );
        }
        assert!(answer(StatusCode::OK, &json!({}), None, true, resource).is_ok());
        assert!(answer(
            StatusCode::OK,
            &json!({"results":[{"resourceName":resource}]}),
            Some("request".into()),
            false,
            resource
        )
        .is_ok());
    }

    #[test]
    fn google_ads_error_details_preserve_the_provider_code_and_uncertainty() {
        use reqwest::StatusCode;
        // Schema examples, not captured responses. Google documents this REST envelope at
        // https://developers.google.com/google-ads/api/docs/best-practices/understand-api-errors.
        for (status, category, code, expected) in [
            (
                400,
                "requestError",
                "RESOURCE_NOT_FOUND",
                AdsError::Refused {
                    code: "RESOURCE_NOT_FOUND".into(),
                },
            ),
            (
                400,
                "campaignBudgetError",
                "BUDGET_AMOUNT_MUST_BE_UNSET_FOR_CUSTOM_BUDGET_PERIOD",
                AdsError::Refused {
                    code: "BUDGET_AMOUNT_MUST_BE_UNSET_FOR_CUSTOM_BUDGET_PERIOD".into(),
                },
            ),
            (
                403,
                "authorizationError",
                "USER_PERMISSION_DENIED",
                AdsError::Access,
            ),
            (
                401,
                "authenticationError",
                "OAUTH_TOKEN_REVOKED",
                AdsError::Access,
            ),
            (429, "quotaError", "RESOURCE_EXHAUSTED", AdsError::Quota),
            (503, "internalError", "TRANSIENT_ERROR", AdsError::Unknown),
        ] {
            let body = json!({"error":{"code":status,"details":[{
                "@type":"type.googleapis.com/google.ads.googleads.v25.errors.GoogleAdsFailure",
                "errors":[{"errorCode":{category:code},"location":{"fieldPathElements":[{"fieldName":"operations","index":0}]}}],
                "requestId":"documented-shape"
            }]}});
            assert_eq!(
                answer(
                    StatusCode::from_u16(status).unwrap(),
                    &body,
                    None,
                    false,
                    ""
                ),
                Err(expected)
            );
        }
        for status in [408, 499, 500, 504] {
            assert_eq!(
                answer(
                    StatusCode::from_u16(status).unwrap(),
                    &json!({}),
                    None,
                    false,
                    ""
                ),
                Err(AdsError::Unknown)
            );
        }
    }

    #[tokio::test]
    #[ignore = "requires an explicitly authorized Google Ads account and short-lived OAuth access token"]
    async fn an_authorized_account_validates_the_exact_budget_and_pause_wire_requests() {
        let customer =
            std::env::var("KEYSEND_CONTRACT_CUSTOMER").expect("authorized customer required");
        assert!(customer.bytes().all(|byte| byte.is_ascii_digit()));
        let access = std::env::var("KEYSEND_CONTRACT_ACCESS_TOKEN")
            .expect("short-lived OAuth token required");
        let campaign: i64 = std::env::var("KEYSEND_CONTRACT_CAMPAIGN")
            .unwrap()
            .parse()
            .unwrap();
        let budget: i64 = std::env::var("KEYSEND_CONTRACT_BUDGET")
            .unwrap()
            .parse()
            .unwrap();
        let amount: i64 = std::env::var("KEYSEND_CONTRACT_BUDGET_MICROS")
            .unwrap()
            .parse()
            .unwrap();
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap();
        for action in [
            Action::AdjustCampaignBudget {
                campaign_id: campaign,
                budget_id: budget,
                from_micros: amount,
                to_micros: amount,
            },
            Action::PauseCampaign {
                campaign_id: campaign,
            },
        ] {
            let (service, operation) = operation(&customer, &action).unwrap();
            let resource = operation["update"]["resourceName"]
                .as_str()
                .unwrap()
                .to_owned();
            let mut request = http.post(format!("https://googleads.googleapis.com/v25/customers/{customer}/{service}:mutate"))
                .bearer_auth(&access).json(&json!({"operations":[operation],"partialFailure":false,"validateOnly":true,"responseContentType":"MUTABLE_RESOURCE"}));
            if let Ok(manager) = std::env::var("KEYSEND_CONTRACT_MANAGER") {
                assert!(manager.bytes().all(|byte| byte.is_ascii_digit()));
                request = request.header("login-customer-id", manager);
            }
            let response = request
                .send()
                .await
                .expect("transport failed; token and body are not logged");
            let status = response.status();
            let request_id = response
                .headers()
                .get("request-id")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            let body: Value = response
                .json()
                .await
                .expect("provider JSON response required");
            assert!(
                answer(status, &body, request_id, true, &resource).is_ok(),
                "provider refused validation, HTTP {}",
                status.as_u16()
            );
        }
    }
}
