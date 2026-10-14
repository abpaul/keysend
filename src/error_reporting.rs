use std::{sync::Arc, time::Duration};

use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use serde_json::json;

use crate::settings::Settings;

pub async fn send(settings: &Settings, operation: &'static str) {
    let Some(key) = settings.postdeploy_key.as_deref() else {
        return;
    };
    let now = crate::settings::wall_clock().to_rfc3339();
    let bytes: [u8; 16] = rand::random();
    let id = bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let body = json!({"header":{"key":key,"sdk":{"name":"keysend","version":env!("CARGO_PKG_VERSION"),"platform":"rust"},"sent_at":now},"items":[{"id":id,"type":"message","timestamp":now,"level":"error","message":operation,"tags":{"application":"KeySend"}}]});
    let accepted = settings
        .http
        .post(&settings.postdeploy.url)
        .bearer_auth(key)
        .timeout(Duration::from_secs(2))
        .json(&body)
        .send()
        .await;
    let accepted = match accepted {
        Ok(response) if response.status() == reqwest::StatusCode::ACCEPTED => response
            .json::<serde_json::Value>()
            .await
            .ok()
            .is_some_and(|body| body["accepted"] == 1 && body["dropped"] == 0),
        _ => false,
    };
    if !accepted {
        tracing::warn!(operation = "error_ingest", "Error reporting failed");
    }
}

pub async fn observe(
    State(settings): State<Arc<Settings>>,
    request: Request,
    next: Next,
) -> Response {
    let response = next.run(request).await;
    if response.status().is_server_error() {
        send(&settings, "HTTP request failed").await;
    }
    response
}
