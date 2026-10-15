use super::*;
use axum::{Router, body::Bytes, http::StatusCode, routing::post};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;

struct Siteverify {
    url: String,
    requests: Arc<Mutex<Vec<HashMap<String, String>>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Siteverify {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn server(status: StatusCode, body: &str, delay: Duration) -> Siteverify {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/siteverify", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::<HashMap<String, String>>::new()));
    let recorded = requests.clone();
    let response = body.to_owned();
    let router = Router::new().route(
        "/siteverify",
        post(move |bytes: Bytes| {
            let recorded = recorded.clone();
            let response = response.clone();
            async move {
                let request = url::form_urlencoded::parse(&bytes)
                    .into_owned()
                    .collect::<HashMap<_, _>>();
                let replay = {
                    let mut requests = recorded.lock().await;
                    let replay = requests
                        .iter()
                        .any(|previous| previous.get("response") == request.get("response"));
                    requests.push(request);
                    replay
                };
                tokio::time::sleep(delay).await;
                if replay {
                    (
                        StatusCode::OK,
                        r#"{"success":false,"error-codes":["timeout-or-duplicate"]}"#.to_owned(),
                    )
                } else {
                    (status, response)
                }
            }
        }),
    );
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    Siteverify {
        url,
        requests,
        task,
    }
}

fn config() -> TurnstileConfig {
    TurnstileConfig {
        enabled: true,
        mode: "managed".into(),
        site_key: "public-site-key".into(),
        secret_key: "siteverify-private-secret".into(),
        hostnames: vec!["blog.example.com".into()],
    }
}

#[tokio::test]
async fn verifies_success_hostname_action_and_posts_server_ip() {
    let site = server(
        StatusCode::OK,
        r#"{"success":true,"hostname":"blog.example.com","action":"comment"}"#,
        Duration::ZERO,
    )
    .await;
    let verifier = TurnstileVerifier::for_test(&site.url, Duration::from_secs(1));
    verifier
        .verify(&config(), Some("one-use-token"), "203.0.113.7")
        .await
        .unwrap();
    let requests = site.requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].get("secret").unwrap(),
        "siteverify-private-secret"
    );
    assert_eq!(requests[0].get("response").unwrap(), "one-use-token");
    assert_eq!(requests[0].get("remoteip").unwrap(), "203.0.113.7");
}

#[tokio::test]
async fn rejects_failed_expired_replayed_or_mismatched_results() {
    for body in [
        r#"{"success":false,"error-codes":["timeout-or-duplicate"]}"#,
        r#"{"success":false,"error-codes":["invalid-input-response"]}"#,
        r#"{"success":true,"hostname":"attacker.example.com","action":"comment"}"#,
        r#"{"success":true,"hostname":"blog.example.com.evil.test","action":"comment"}"#,
        r#"{"success":true,"hostname":"blog.example.com","action":"login"}"#,
        r#"{"success":true,"hostname":"blog.example.com"}"#,
        r#"{"success":true,"action":"comment"}"#,
    ] {
        let site = server(StatusCode::OK, body, Duration::ZERO).await;
        let verifier = TurnstileVerifier::for_test(&site.url, Duration::from_secs(1));
        assert!(
            matches!(
                verifier
                    .verify(&config(), Some("token"), "203.0.113.7")
                    .await,
                Err(AppError::BadRequest(_))
            ),
            "{body}"
        );
    }
}

#[tokio::test]
async fn rejects_missing_oversized_tokens_and_broken_secrets_without_network() {
    let site = server(StatusCode::OK, "{}", Duration::ZERO).await;
    let verifier = TurnstileVerifier::for_test(&site.url, Duration::from_secs(1));
    for token in [None, Some(""), Some("  "), Some(&"x".repeat(2049))] {
        assert!(
            verifier
                .verify(&config(), token, "203.0.113.7")
                .await
                .is_err()
        );
    }
    let mut broken = config();
    broken.secret_key.clear();
    assert!(
        verifier
            .verify(&broken, Some("token"), "203.0.113.7")
            .await
            .is_err()
    );
    broken = config();
    broken.hostnames.clear();
    assert!(
        verifier
            .verify(&broken, Some("token"), "203.0.113.7")
            .await
            .is_err()
    );
    assert!(site.requests.lock().await.is_empty());
}

#[tokio::test]
async fn disabled_turnstile_makes_no_external_request() {
    let site = server(StatusCode::OK, "{}", Duration::ZERO).await;
    let verifier = TurnstileVerifier::for_test(&site.url, Duration::from_secs(1));
    let mut disabled = config();
    disabled.enabled = false;
    verifier
        .verify(&disabled, None, "203.0.113.7")
        .await
        .unwrap();
    assert!(site.requests.lock().await.is_empty());
}

#[tokio::test]
async fn rejects_upstream_http_bad_json_oversized_body_and_timeout_without_leaking_secrets() {
    for (status, body, delay) in [
        (
            StatusCode::BAD_GATEWAY,
            "upstream-private-detail".to_owned(),
            Duration::ZERO,
        ),
        (
            StatusCode::OK,
            "invalid-private-json".to_owned(),
            Duration::ZERO,
        ),
        (StatusCode::OK, "x".repeat(17_000), Duration::ZERO),
        (
            StatusCode::TEMPORARY_REDIRECT,
            "".to_owned(),
            Duration::ZERO,
        ),
        (StatusCode::OK, "{}".to_owned(), Duration::from_millis(200)),
    ] {
        let site = server(status, &body, delay).await;
        let verifier = TurnstileVerifier::for_test(&site.url, Duration::from_millis(50));
        let error = verifier
            .verify(&config(), Some("private-token"), "203.0.113.7")
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "internal server error");
        let debug = format!("{error:?}");
        for private in [
            "siteverify-private-secret",
            "private-token",
            "upstream-private-detail",
            "invalid-private-json",
        ] {
            assert!(!debug.contains(private));
        }
    }
    let verifier =
        TurnstileVerifier::for_test("http://127.0.0.1:0/siteverify", Duration::from_millis(50));
    assert!(
        verifier
            .verify(&config(), Some("token"), "203.0.113.7")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn accepted_token_creates_comment_and_mail_once_and_replay_cannot_write() {
    use crate::{
        config::{CommentConfig, MailConfig},
        dto::CommentCreateRequest,
        entities::{comments, mail_jobs},
        services::{captcha_service::CaptchaMemoryStore, comment_service},
    };
    use migration::{Migrator, MigratorTrait};
    use sea_orm::{ConnectOptions, Database, EntityTrait, PaginatorTrait};
    let site = server(
        StatusCode::OK,
        r#"{"success":true,"hostname":"blog.example.com","action":"comment"}"#,
        Duration::ZERO,
    )
    .await;
    let verifier = TurnstileVerifier::for_test(&site.url, Duration::from_secs(1));
    let mut options = ConnectOptions::new("sqlite::memory:");
    options.max_connections(1);
    let db = Database::connect(options).await.unwrap();
    Migrator::up(&db, None).await.unwrap();
    let mut comment = CommentConfig::default();
    comment.captcha.pow_enabled = false;
    comment.captcha.image_enabled = false;
    comment.captcha.turnstile = config();
    let mail = MailConfig {
        enabled: true,
        notify_admin: true,
        admin_email: "admin@example.test".into(),
        ..Default::default()
    };
    let memory = CaptchaMemoryStore::new();
    for expected in [true, false] {
        let payload: CommentCreateRequest = serde_json::from_value(serde_json::json!({
            "url":"/article", "comment":"verified comment", "turnstile_token":"one-use-token"
        }))
        .unwrap();
        let result = comment_service::create_comment(
            &db,
            &None,
            &memory,
            &[],
            &verifier,
            &comment,
            &mail,
            payload,
            "203.0.113.7".into(),
            None,
        )
        .await;
        assert_eq!(result.is_ok(), expected);
        assert_eq!(comments::Entity::find().count(&db).await.unwrap(), 1);
        assert_eq!(mail_jobs::Entity::find().count(&db).await.unwrap(), 1);
    }
    assert_eq!(site.requests.lock().await.len(), 2);
    let trap = serde_json::from_value(serde_json::json!({
        "url":"/article", "comment":"bot", "hp":"trap", "turnstile_token":"never-consume"
    }))
    .unwrap();
    comment_service::create_comment(
        &db,
        &None,
        &memory,
        &[],
        &verifier,
        &comment,
        &mail,
        trap,
        "203.0.113.7".into(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(site.requests.lock().await.len(), 2);
    assert_eq!(comments::Entity::find().count(&db).await.unwrap(), 1);
}
