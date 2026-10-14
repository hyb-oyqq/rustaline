//! Turnstile 的公共 API 契约；不依赖 Cloudflare 网络或生产密钥。
use std::sync::Arc;

use app::{
    config::AppConfig,
    middleware::rate_limit::RateLimiter,
    services::{captcha_service, settings_service},
    state::AppState,
};
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use migration::{Migrator, MigratorTrait};
use sea_orm::{ConnectOptions, Database, EntityTrait, PaginatorTrait};
use serde_json::{Value, json};
use tower::ServiceExt;

async fn app() -> (Router, AppState) {
    let mut config: AppConfig = config::Config::builder()
        .add_source(config::File::from_str(
            include_str!("../../config/default.toml"),
            config::FileFormat::Toml,
        ))
        .build()
        .unwrap()
        .try_deserialize()
        .unwrap();
    config.jwt.blacklist_enabled = false;
    config.comment.captcha.pow_enabled = false;
    config.comment.captcha.image_enabled = false;
    let mut options = ConnectOptions::new("sqlite::memory:");
    options.max_connections(1);
    let db = Database::connect(options).await.unwrap();
    Migrator::up(&db, None).await.unwrap();
    settings_service::initialize(&db, &config).await.unwrap();
    let key = captcha_service::signing_key(&config.comment.captcha, &config.jwt.secret);
    let state = AppState {
        db,
        redis: None,
        config: Arc::new(config),
        comment_rate_limiter: RateLimiter::new(),
        login_rate_limiter: RateLimiter::new(),
        pow_issue_rate_limiter: RateLimiter::new(),
        image_issue_rate_limiter: RateLimiter::new(),
        captcha_memory: captcha_service::CaptchaMemoryStore::new(),
        captcha_signing_key: Arc::new(key),
        turnstile: app::services::turnstile_service::TurnstileVerifier::new().unwrap(),
    };
    (app::routes::create_router(state.clone()), state)
}

async fn call(router: &Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

#[tokio::test]
async fn turnstile_public_config_disables_legacy_settings() {
    let (router, _) = app().await;
    let (status, data) = call(&router, "GET", "/api/v1/captcha/config", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(data["turnstile"]["enabled"], false);
    assert_eq!(data["turnstile"]["mode"], "managed");
    assert!(!data.to_string().contains("secret"));
    let (status, _) = call(
        &router,
        "POST",
        "/api/v1/comments",
        json!({"url":"/legacy", "comment":"compatible old client"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
}

async fn enable(state: &AppState) {
    let patch = serde_json::from_value(json!({
        "revision":1,
        "comment":{"captcha":{"turnstile":{
            "enabled":true, "mode":"managed", "site_key":"public-site-key",
            "secret_key":"private-turnstile-secret", "hostnames":["blog.example.com"]
        }}}
    }))
    .unwrap();
    settings_service::update(&state.db, &state.config.jwt.secret, patch)
        .await
        .unwrap();
}

#[tokio::test]
async fn turnstile_enabled_config_is_public_but_missing_token_cannot_create_comment() {
    let (router, state) = app().await;
    enable(&state).await;
    let (_, config) = call(&router, "GET", "/api/v1/captcha/config", Value::Null).await;
    assert_eq!(config["turnstile"]["enabled"], true);
    assert_eq!(config["turnstile"]["site_key"], "public-site-key");
    assert!(!config.to_string().contains("private-turnstile-secret"));
    assert!(config["turnstile"].get("hostnames").is_none());
    for token in [Value::Null, json!(""), json!("x".repeat(2049))] {
        let (status, body) = call(
            &router,
            "POST",
            "/api/v1/comments",
            json!({"url":"/protected", "comment":"cannot bypass", "turnstile_token":token}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(!body.to_string().contains("private-turnstile-secret"));
    }
    assert_eq!(
        app::entities::comments::Entity::find()
            .count(&state.db)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        app::entities::mail_jobs::Entity::find()
            .count(&state.db)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn turnstile_honeypot_keeps_fake_success_without_creating_comment() {
    let (router, state) = app().await;
    enable(&state).await;
    let (status, _) = call(
        &router,
        "POST",
        "/api/v1/comments",
        json!({"url":"/trap", "comment":"bot", "hp":"filled"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        app::entities::comments::Entity::find()
            .count(&state.db)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn turnstile_rotated_jwt_keeps_protection_enabled_and_rejects_submissions() {
    let (_, mut state) = app().await;
    enable(&state).await;
    let mut config = (*state.config).clone();
    config.jwt.secret = "rotated-key-that-cannot-decrypt-turnstile".into();
    state.config = Arc::new(config);
    let router = app::routes::create_router(state.clone());
    let (_, public) = call(&router, "GET", "/api/v1/captcha/config", Value::Null).await;
    assert_eq!(public["turnstile"]["enabled"], true);
    let (status, body) = call(
        &router,
        "POST",
        "/api/v1/comments",
        json!({"url":"/protected", "comment":"blocked until repaired", "turnstile_token":"token"}),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["message"], "internal server error");
    assert_eq!(
        app::entities::comments::Entity::find()
            .count(&state.db)
            .await
            .unwrap(),
        0
    );
}
