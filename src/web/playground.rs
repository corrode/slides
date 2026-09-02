use std::{sync::LazyLock, time::Duration};

use axum::{Json, http::StatusCode};
use reqwest::Client;
use serde::{Deserialize, Serialize};

const MAX_CODE_BYTES: usize = 64 * 1024;
const PLAYGROUND_TIMEOUT: Duration = Duration::from_secs(20);
static PLAYGROUND_CLIENT: LazyLock<Client> = LazyLock::new(Client::new);

#[derive(Debug, Deserialize)]
pub struct RunRequest {
    code: String,
}

#[derive(Debug, Deserialize)]
struct PlaygroundResponse {
    success: bool,
    stdout: String,
    stderr: String,
}

#[derive(Debug, Serialize)]
pub struct RunResponse {
    success: bool,
    stdout: String,
    stderr: String,
}

/// Proxies Rust source to the official Playground, where untrusted code runs in
/// its sandbox rather than in the Slides process.
pub async fn run(Json(request): Json<RunRequest>) -> Result<Json<RunResponse>, StatusCode> {
    if request.code.len() > MAX_CODE_BYTES {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }

    let response = PLAYGROUND_CLIENT
        .post("https://play.rust-lang.org/execute")
        .timeout(PLAYGROUND_TIMEOUT)
        .json(&serde_json::json!({
            "channel": "stable",
            "mode": "debug",
            "edition": "2024",
            "crateType": "bin",
            "tests": false,
            "backtrace": false,
            "code": request.code,
        }))
        .send()
        .await
        .map_err(|error| {
            tracing::warn!(?error, "Rust Playground request failed");
            StatusCode::BAD_GATEWAY
        })?;

    if !response.status().is_success() {
        let status = response.status();
        tracing::warn!(%status, "Rust Playground returned an error");
        return Err(if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            StatusCode::TOO_MANY_REQUESTS
        } else {
            StatusCode::BAD_GATEWAY
        });
    }

    let response = response
        .json::<PlaygroundResponse>()
        .await
        .map_err(|error| {
            tracing::warn!(?error, "Rust Playground returned an invalid response");
            StatusCode::BAD_GATEWAY
        })?;

    Ok(Json(RunResponse {
        success: response.success,
        stdout: response.stdout,
        stderr: response.stderr,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_oversized_source_without_contacting_the_playground() {
        let request = RunRequest {
            code: "x".repeat(MAX_CODE_BYTES + 1),
        };

        let status = run(Json(request)).await.unwrap_err();

        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    }
}
