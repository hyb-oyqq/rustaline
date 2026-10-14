use super::*;
use migration::{Migrator, MigratorTrait};
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DbBackend, PaginatorTrait, Statement};

fn turnstile_patch(revision: i64, value: serde_json::Value) -> SettingsPatchRequest {
    serde_json::from_value(serde_json::json!({
        "revision": revision, "comment": {"captcha": {"turnstile": value}}
    }))
    .unwrap()
}

fn enabled_turnstile() -> serde_json::Value {
    serde_json::json!({"enabled": true, "mode": "managed", "site_key": "site-key",
        "secret_key": "  private-value  ", "hostnames": [" Comments.Example.com ", "comments.example.com"]})
}

#[tokio::test]
async fn turnstile_secret_is_independent_random_and_preserved() {
    let db = database().await;
    let mut config = bootstrap();
    config.mail.username = "user".into();
    config.mail.password = "  private-value  ".into();
    initialize(&db, &config).await.unwrap();
    let first = update(
        &db,
        &config.jwt.secret,
        turnstile_patch(1, enabled_turnstile()),
    )
    .await
    .unwrap();
    assert_eq!(
        first.comment.captcha.turnstile.secret_key,
        "  private-value  "
    );
    assert_eq!(
        first.comment.captcha.turnstile.hostnames,
        ["comments.example.com"]
    );
    assert!(!format!("{first:?}").contains("private-value"));
    let stored = row(&db).await.unwrap();
    assert!(!stored.content.contains("private-value"));
    assert!(!stored.content.contains("secret"));
    let cipher = stored.turnstile_secret_ciphertext.unwrap();
    assert!(settings_crypto::decrypt(&cipher, &config.jwt.secret).is_none());
    assert!(
        settings_crypto::decrypt_turnstile(
            stored.smtp_password_ciphertext.as_ref().unwrap(),
            &config.jwt.secret
        )
        .is_none()
    );
    update(
        &db,
        &config.jwt.secret,
        turnstile_patch(2, serde_json::json!({"secret_key": "  private-value  "})),
    )
    .await
    .unwrap();
    let second = row(&db).await.unwrap().turnstile_secret_ciphertext;
    assert_ne!(Some(cipher), second);
    update(
        &db,
        &config.jwt.secret,
        turnstile_patch(3, serde_json::json!({"secret_key": ""})),
    )
    .await
    .unwrap();
    assert_eq!(row(&db).await.unwrap().turnstile_secret_ciphertext, second);
    update(
        &db,
        &config.jwt.secret,
        turnstile_patch(4, serde_json::json!({"mode": "click"})),
    )
    .await
    .unwrap();
    assert_eq!(row(&db).await.unwrap().turnstile_secret_ciphertext, second);
    assert!(matches!(
        update(
            &db,
            &config.jwt.secret,
            turnstile_patch(4, serde_json::json!({"mode":"invisible"}))
        )
        .await,
        Err(AppError::Conflict(_))
    ));
}

#[tokio::test]
async fn turnstile_jwt_rotation_keeps_protection_and_allows_repair() {
    let db = database().await;
    let mut config = bootstrap();
    initialize(&db, &config).await.unwrap();
    update(
        &db,
        &config.jwt.secret,
        turnstile_patch(1, enabled_turnstile()),
    )
    .await
    .unwrap();
    let cipher = row(&db).await.unwrap().turnstile_secret_ciphertext;
    config.jwt.secret = "rotated-jwt-secret".into();
    let snapshot = initialize(&db, &config).await.unwrap();
    assert!(snapshot.comment.captcha.turnstile.enabled);
    assert!(snapshot.turnstile_secret_needs_reset);
    assert!(snapshot.comment.captcha.turnstile.secret_key.is_empty());
    let response = serde_json::to_value(response(&config, &snapshot)).unwrap();
    assert_eq!(
        response["comment"]["captcha"]["turnstile"]["secret_configured"],
        true
    );
    assert_eq!(
        response["comment"]["captcha"]["turnstile"]["secret_needs_reset"],
        true
    );
    let patch =
        serde_json::from_value(serde_json::json!({"revision":2,"comment":{"moderation":true}}))
            .unwrap();
    assert!(
        update(&db, &config.jwt.secret, patch)
            .await
            .unwrap()
            .turnstile_secret_needs_reset
    );
    update(
        &db,
        &config.jwt.secret,
        turnstile_patch(3, serde_json::json!({"secret_key":"", "mode":"click"})),
    )
    .await
    .unwrap();
    assert_eq!(row(&db).await.unwrap().turnstile_secret_ciphertext, cipher);
    let fixed = update(
        &db,
        &config.jwt.secret,
        turnstile_patch(4, serde_json::json!({"secret_key":"replacement"})),
    )
    .await
    .unwrap();
    assert!(!fixed.turnstile_secret_needs_reset);
    assert!(fixed.comment.captcha.turnstile.enabled);
    assert_eq!(
        load(&db, &config.jwt.secret)
            .await
            .unwrap()
            .comment
            .captcha
            .turnstile
            .secret_key,
        "replacement"
    );
}

#[tokio::test]
async fn turnstile_invalid_settings_are_rejected_atomically() {
    let db = database().await;
    let config = bootstrap();
    initialize(&db, &config).await.unwrap();
    for value in [
        serde_json::json!({"enabled":true}),
        serde_json::json!({"mode":"automatic"}),
        serde_json::json!({"site_key":"bad key"}),
        serde_json::json!({"site_key":"x".repeat(256)}),
        serde_json::json!({"secret_key":"x".repeat(4097)}),
        serde_json::json!({"hostnames":vec!["example.com";101]}),
    ] {
        assert!(matches!(
            update(&db, &config.jwt.secret, turnstile_patch(1, value)).await,
            Err(AppError::BadRequest(_))
        ));
    }
    for hostname in [
        "",
        "https://example.com",
        "example.com:443",
        "example.com/path",
        "*.example.com",
        "bad host",
        "-bad.com",
        "bad..com",
        &"x".repeat(254),
    ] {
        assert!(
            matches!(
                update(
                    &db,
                    &config.jwt.secret,
                    turnstile_patch(1, serde_json::json!({"hostnames":[hostname]}))
                )
                .await,
                Err(AppError::BadRequest(_))
            ),
            "{hostname}"
        );
    }
    assert_eq!(row(&db).await.unwrap().revision, 1);
    assert!(
        row(&db)
            .await
            .unwrap()
            .turnstile_secret_ciphertext
            .is_none()
    );
}

#[tokio::test]
async fn turnstile_upgrade_from_six_migrations_preserves_existing_settings() {
    let db = Database::connect("sqlite::memory:").await.unwrap();
    Migrator::up(&db, Some(6)).await.unwrap();
    let legacy = r#"{"comment":{"moderation":true},"mail":{"site_name":"legacy"}}"#;
    db.execute_raw(Statement::from_sql_and_values(DbBackend::Sqlite,
        "INSERT INTO app_settings (id,schema_version,revision,content,smtp_password_ciphertext,created_at,updated_at) VALUES (1,1,7,?,NULL,'2026-10-01 00:00:00','2026-10-01 00:00:00')", [legacy.into()])).await.unwrap();
    Migrator::up(&db, None).await.unwrap();
    let mut config = bootstrap();
    config.comment.captcha.turnstile.enabled = true;
    let snapshot = initialize(&db, &config).await.unwrap();
    assert_eq!(snapshot.revision, 7);
    assert!(snapshot.comment.moderation);
    assert_eq!(snapshot.mail.site_name, "legacy");
    assert!(!snapshot.comment.captcha.turnstile.enabled);
    assert_eq!(snapshot.comment.captcha.turnstile.mode, "managed");
    assert_eq!(row(&db).await.unwrap().content, legacy);
    assert!(
        row(&db)
            .await
            .unwrap()
            .turnstile_secret_ciphertext
            .is_none()
    );
}

#[tokio::test]
async fn turnstile_bootstrap_seeds_once_and_normalizes_hostnames() {
    let db = database().await;
    let mut config = bootstrap();
    config.comment.captcha.turnstile = serde_json::from_value(enabled_turnstile()).unwrap();
    let first = initialize(&db, &config).await.unwrap();
    assert!(first.comment.captcha.turnstile.enabled);
    assert_eq!(
        first.comment.captcha.turnstile.hostnames,
        ["comments.example.com"]
    );
    config.comment.captcha.turnstile.secret_key = "ignored".into();
    assert_eq!(
        initialize(&db, &config)
            .await
            .unwrap()
            .comment
            .captcha
            .turnstile
            .secret_key,
        "  private-value  "
    );
}

#[test]
fn seed_insert_sql_preserves_mysql_and_other_backend_conflict_semantics() {
    use sea_orm::QueryTrait;
    let model = app_settings::ActiveModel {
        id: Set(1),
        ..Default::default()
    };
    let mysql = seed_insert(model.clone())
        .build(DbBackend::MySql)
        .to_string();
    assert!(
        mysql.ends_with("ON DUPLICATE KEY UPDATE `id` = `id`"),
        "{mysql}"
    );
    for backend in [DbBackend::Sqlite, DbBackend::Postgres] {
        let sql = seed_insert(model.clone()).build(backend).to_string();
        assert!(sql.ends_with("ON CONFLICT (\"id\") DO NOTHING"), "{sql}");
    }
}

#[tokio::test]
async fn captcha_ttl_uses_a_stable_supported_lifetime() {
    let db = database().await;
    let config = bootstrap();
    initialize(&db, &config).await.unwrap();
    let invalid = serde_json::from_value(serde_json::json!({
        "revision": 1, "comment": {"captcha": {"pow_ttl_secs": 31_536_001}}
    }))
    .unwrap();
    assert!(matches!(
        update(&db, &config.jwt.secret, invalid).await,
        Err(AppError::BadRequest(_))
    ));
    let valid = serde_json::from_value(serde_json::json!({
        "revision": 1, "comment": {"captcha": {"pow_ttl_secs": 31_536_000, "image_ttl_secs": 31_536_000}}
    })).unwrap();
    update(&db, &config.jwt.secret, valid).await.unwrap();
    let restarted = initialize(&db, &config).await.unwrap();
    assert_eq!(restarted.comment.captcha.pow_ttl_secs, 31_536_000);
}

async fn database() -> DatabaseConnection {
    let mut options = ConnectOptions::new("sqlite::memory:");
    options.max_connections(1);
    let db = Database::connect(options).await.unwrap();
    Migrator::up(&db, None).await.unwrap();
    db
}

fn bootstrap() -> AppConfig {
    config::Config::builder()
        .add_source(config::File::from_str(
            include_str!("../../../../config/default.toml"),
            config::FileFormat::Toml,
        ))
        .build()
        .unwrap()
        .try_deserialize()
        .unwrap()
}

#[tokio::test]
async fn initialization_is_idempotent_and_restart_reads_database() {
    let db = database().await;
    let mut config = bootstrap();
    config.comment.moderation = true;
    config.comment.captcha.secret = "deployment-secret".into();
    config.mail.password = "smtp-secret".into();
    config.mail.username = "smtp-user".into();
    let first = initialize(&db, &config).await.unwrap();
    assert_eq!(first.revision, 1);
    assert!(first.comment.moderation);
    config.comment.moderation = false;
    config.comment.max_length = 0;
    let second = initialize(&db, &config).await.unwrap();
    assert!(second.comment.moderation);
    assert_eq!(second.comment.max_length, 10000);
    assert_eq!(second.mail.password, "smtp-secret");
    let row = app_settings::Entity::find_by_id(1)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert!(!row.content.contains("smtp-secret"));
    assert!(!row.content.contains("deployment-secret"));
    assert!(!row.content.contains("password"));
    assert!(!row.content.contains("secret"));
    assert!(
        !row.smtp_password_ciphertext
            .unwrap()
            .contains("smtp-secret")
    );
    assert!(!format!("{:?}", second.mail).contains("smtp-secret"));
}

#[tokio::test]
async fn concurrent_initialization_creates_one_row() {
    let db = database().await;
    let config = bootstrap();
    let (left, right) = tokio::join!(initialize(&db, &config), initialize(&db, &config));
    assert_eq!(left.unwrap().revision, 1);
    assert_eq!(right.unwrap().revision, 1);
    assert_eq!(app_settings::Entity::find().count(&db).await.unwrap(), 1);
}

#[tokio::test]
async fn defaults_fill_missing_fields_and_future_schema_is_rejected() {
    let db = database().await;
    let config = bootstrap();
    initialize(&db, &config).await.unwrap();
    app_settings::Entity::update_many()
        .col_expr(
            app_settings::Column::Content,
            Expr::value(r#"{"comment":{"moderation":true}}"#),
        )
        .filter(app_settings::Column::Id.eq(1))
        .exec(&db)
        .await
        .unwrap();
    let snapshot = load(&db, &config.jwt.secret).await.unwrap();
    assert!(snapshot.comment.moderation);
    assert_eq!(snapshot.comment.max_length, 10000);
    assert_eq!(snapshot.mail.port, 587);
    assert_eq!(snapshot.mail.security, "starttls");
    assert_eq!(snapshot.mail.language, "zh-CN");
    app_settings::Entity::update_many()
        .col_expr(app_settings::Column::SchemaVersion, Expr::value(2))
        .filter(app_settings::Column::Id.eq(1))
        .exec(&db)
        .await
        .unwrap();
    assert!(initialize(&db, &config).await.is_err());
    assert_eq!(
        app_settings::Entity::find_by_id(1)
            .one(&db)
            .await
            .unwrap()
            .unwrap()
            .schema_version,
        2
    );
}

#[tokio::test]
async fn changed_jwt_secret_preserves_password_ciphertext_until_replaced_or_cleared() {
    let db = database().await;
    let mut config = bootstrap();
    config.mail.username = "smtp-user".into();
    config.mail.password = " first secret ".into();
    initialize(&db, &config).await.unwrap();
    let cipher = app_settings::Entity::find_by_id(1)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .smtp_password_ciphertext;
    let changed_secret = "different-jwt-secret";
    let snapshot = load(&db, changed_secret).await.unwrap();
    assert!(snapshot.password_needs_reset);
    assert!(snapshot.mail.password.is_empty());
    let patch =
        serde_json::from_value(serde_json::json!({"revision":1,"mail":{"site_name":"Updated"}}))
            .unwrap();
    let updated = update(&db, changed_secret, patch).await.unwrap();
    assert!(updated.password_needs_reset);
    assert_eq!(
        app_settings::Entity::find_by_id(1)
            .one(&db)
            .await
            .unwrap()
            .unwrap()
            .smtp_password_ciphertext,
        cipher
    );
    let patch = serde_json::from_value(
        serde_json::json!({"revision":2,"mail":{"password":"new password"}}),
    )
    .unwrap();
    let updated = update(&db, changed_secret, patch).await.unwrap();
    assert!(!updated.password_needs_reset);
    assert_eq!(updated.mail.password, "new password");
    let patch =
        serde_json::from_value(serde_json::json!({"revision":3,"mail":{"clear_password":true}}))
            .unwrap();
    assert!(
        update(&db, changed_secret, patch)
            .await
            .unwrap()
            .mail
            .password
            .is_empty()
    );
    assert!(
        app_settings::Entity::find_by_id(1)
            .one(&db)
            .await
            .unwrap()
            .unwrap()
            .smtp_password_ciphertext
            .is_none()
    );
}

#[tokio::test]
async fn conditional_update_allows_only_one_writer() {
    let db = database().await;
    let config = bootstrap();
    initialize(&db, &config).await.unwrap();
    let left =
        serde_json::from_value(serde_json::json!({"revision":1,"comment":{"moderation":true}}))
            .unwrap();
    let right =
        serde_json::from_value(serde_json::json!({"revision":1,"comment":{"moderation":false}}))
            .unwrap();
    let (left, right) = tokio::join!(
        update(&db, &config.jwt.secret, left),
        update(&db, &config.jwt.secret, right)
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    let rejected = left.err().or(right.err()).unwrap();
    assert!(matches!(rejected, AppError::Conflict(_)));
    assert_eq!(load(&db, &config.jwt.secret).await.unwrap().revision, 2);
}

#[tokio::test]
async fn redis_expiration_overflow_is_rejected_before_saving() {
    let db = database().await;
    let config = bootstrap();
    initialize(&db, &config).await.unwrap();
    for field in ["pow_ttl_secs", "image_ttl_secs"] {
        let patch = serde_json::from_value(serde_json::json!({
            "revision": 1,
            "comment": {"captcha": {field: 9_223_372_036_854_775_u64}}
        }))
        .unwrap();
        assert!(matches!(
            update(&db, &config.jwt.secret, patch).await,
            Err(AppError::BadRequest(_))
        ));
    }
    assert_eq!(load(&db, &config.jwt.secret).await.unwrap().revision, 1);
}

#[tokio::test]
async fn legacy_zero_length_and_empty_nickname_survive_upgrade_and_mail_edits() {
    let db = database().await;
    let mut config = bootstrap();
    config.comment.max_length = 0;
    config.comment.default_nick.clear();
    let first = initialize(&db, &config).await.unwrap();
    assert_eq!(first.comment.max_length, 0);
    assert!(first.comment.default_nick.is_empty());
    let patch = serde_json::from_value(serde_json::json!({
        "revision": 1, "mail": {"site_name": "Updated"}
    }))
    .unwrap();
    update(&db, &config.jwt.secret, patch).await.unwrap();
    let restarted = initialize(&db, &config).await.unwrap();
    assert_eq!(restarted.comment.max_length, 0);
    assert!(restarted.comment.default_nick.is_empty());
    let patch = serde_json::from_value(serde_json::json!({
        "revision": 2, "comment": {"moderation": true}
    }))
    .unwrap();
    assert!(matches!(
        update(&db, &config.jwt.secret, patch).await,
        Err(AppError::BadRequest(_))
    ));
}

#[tokio::test]
async fn upgrade_preserves_existing_comments_and_admin_credentials() {
    let mut options = ConnectOptions::new("sqlite::memory:");
    options.max_connections(1);
    let db = Database::connect(options).await.unwrap();
    Migrator::up(&db, Some(4)).await.unwrap();
    db.execute_raw(Statement::from_string(DbBackend::Sqlite,
        "INSERT INTO comments (id, comment, nick, url, inserted_at, created_at, updated_at) VALUES ('old-comment', 'kept body', 'Old author', '/old', '2026-10-01 00:00:00', '2026-10-01 00:00:00', '2026-10-01 00:00:00')")).await.unwrap();
    db.execute_raw(Statement::from_string(DbBackend::Sqlite,
        "INSERT INTO admins (id, username, password_hash, token_version, created_at, updated_at) VALUES ('old-admin', 'original-admin', 'original-password-hash', 7, '2026-10-01 00:00:00', '2026-10-01 00:00:00')")).await.unwrap();
    Migrator::up(&db, None).await.unwrap();
    let mut config = bootstrap();
    config.comment.default_nick = "Migrated nickname".into();
    initialize(&db, &config).await.unwrap();
    let comment = crate::entities::comments::Entity::find_by_id("old-comment")
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(comment.comment, "kept body");
    let admin = crate::entities::admins::Entity::find_by_id("old-admin")
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(admin.username, "original-admin");
    assert_eq!(admin.password_hash, "original-password-hash");
    assert_eq!(admin.token_version, 7);
    assert_eq!(
        load(&db, &config.jwt.secret)
            .await
            .unwrap()
            .comment
            .default_nick,
        "Migrated nickname"
    );
}

#[tokio::test]
async fn password_encryption_uses_random_nonce_and_preserves_exact_whitespace() {
    let db = database().await;
    let config = bootstrap();
    initialize(&db, &config).await.unwrap();
    let patch = serde_json::from_value(
        serde_json::json!({"revision":1,"mail":{"username":"user","password":"  same password  "}}),
    )
    .unwrap();
    update(&db, &config.jwt.secret, patch).await.unwrap();
    let first = app_settings::Entity::find_by_id(1)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .smtp_password_ciphertext;
    let patch = serde_json::from_value(
        serde_json::json!({"revision":2,"mail":{"password":"  same password  "}}),
    )
    .unwrap();
    update(&db, &config.jwt.secret, patch).await.unwrap();
    let second = app_settings::Entity::find_by_id(1)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .smtp_password_ciphertext;
    assert_ne!(first, second);
    assert_eq!(
        load(&db, &config.jwt.secret).await.unwrap().mail.password,
        "  same password  "
    );
    let patch =
        serde_json::from_value(serde_json::json!({"revision":3,"mail":{"password":""}})).unwrap();
    update(&db, &config.jwt.secret, patch).await.unwrap();
    assert_eq!(
        app_settings::Entity::find_by_id(1)
            .one(&db)
            .await
            .unwrap()
            .unwrap()
            .smtp_password_ciphertext,
        second
    );
}
