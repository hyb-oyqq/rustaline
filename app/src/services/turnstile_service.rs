//! Turnstile token 只能由评论后端在固定 Siteverify 地址消费一次。
use std::time::Duration;

use serde::Deserialize;

use crate::{config::TurnstileConfig, error::AppError};

const SITEVERIFY_URL: &str = "https://challenges.cloudflare.com/turnstile/v0/siteverify";
const MAX_RESPONSE_BYTES: usize = 16 * 1024;

#[derive(Clone)]
pub struct TurnstileVerifier {
    client: reqwest::Client,
    endpoint: String,
}

#[derive(Deserialize)]
struct VerificationResponse {
    success: bool,
    hostname: Option<String>,
    action: Option<String>,
}

fn unavailable() -> AppError {
    // 不携带上游正文、请求参数、token 或密钥，连内部 Debug 也无法泄漏。
    AppError::internal_msg("turnstile verification unavailable")
}

impl TurnstileVerifier {
    pub fn new() -> Result<Self, AppError> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| unavailable())?;
        Ok(Self {
            client,
            endpoint: SITEVERIFY_URL.into(),
        })
    }

    pub async fn verify(
        &self,
        config: &TurnstileConfig,
        token: Option<&str>,
        ip: &str,
    ) -> Result<(), AppError> {
        if !config.enabled {
            return Ok(());
        }
        let token = token
            .filter(|token| !token.trim().is_empty() && token.len() <= 2048)
            .ok_or_else(|| AppError::BadRequest("turnstile token is missing or invalid".into()))?;
        if config.secret_key.trim().is_empty() || config.hostnames.is_empty() {
            return Err(unavailable());
        }
        let mut response = self
            .client
            .post(&self.endpoint)
            .form(&[
                ("secret", config.secret_key.as_str()),
                ("response", token),
                ("remoteip", ip),
            ])
            .send()
            .await
            .map_err(|_| unavailable())?;
        if !response.status().is_success()
            || response
                .content_length()
                .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
        {
            return Err(unavailable());
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
            if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                return Err(unavailable());
            }
            body.extend_from_slice(&chunk);
        }
        let result: VerificationResponse =
            serde_json::from_slice(&body).map_err(|_| unavailable())?;
        if !result.success
            || result.action.as_deref() != Some("comment")
            || !config
                .hostnames
                .iter()
                .any(|hostname| result.hostname.as_deref() == Some(hostname))
        {
            return Err(AppError::BadRequest(
                "turnstile verification failed; retry with a fresh token".into(),
            ));
        }
        Ok(())
    }

    #[cfg(test)]
    fn for_test(endpoint: &str, timeout: Duration) -> Self {
        Self {
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(timeout)
                .build()
                .unwrap(),
            endpoint: endpoint.into(),
        }
    }
}

#[cfg(test)]
mod tests;
