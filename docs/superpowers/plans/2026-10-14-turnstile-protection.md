# Turnstile 评论防护实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox syntax for tracking.

**Goal:** 后台配置 Turnstile，并在评论和回复提交时支持隐藏、自适应、必须点击三种交互。

**Architecture:** 业务配置沿用数据库快照与 revision。新增独立加密密文列，固定 Siteverify 验证服务只在已有评论服务中调用；SDK 共享官方脚本但隔离每实例组件，后台沿用文件夹式设置。

**Tech Stack:** Rust/Axum/SeaORM、SQLite/PostgreSQL/MySQL、reqwest + rustls、AES-GCM、原生 JavaScript 与本地 mdui。

**Spec:** `docs/superpowers/specs/2026-10-14-turnstile-protection-design.md`（用户已确认）。

## Global Constraints

- Turnstile 默认关闭；三种模式为 `invisible`、`managed`、`click`，默认 `managed`。
- Turnstile 与 PoW、图形码独立开关，同开为 AND。
- PATCH 密钥省略或空字符串保留；禁止回显秘密、禁止增加清除密钥按钮。
- `success == true`、action 精确等于 `comment`、hostname 精确匹配管理员列表；token 非空且最多 2048 字符。
- Siteverify 生产 URL 固定为 `https://challenges.cloudflare.com/turnstile/v0/siteverify`，总超时 10 秒，禁止重定向，失败不能放行。
- 密钥轮换失败不自动关闭防护；启动、后台访问和其他设置继续可用。
- SDK 单文件原生 JS，官方 Turnstile 脚本按需加载；每次提交先刷新配置。
- 所有新界面文案中英双语，用户内容只用安全 DOM；不修改 vendor。
- 不自动创建 Cloudflare 组件，不获取生产密钥，不自动发布部署。

## Task 1: 数据库迁移与运行时设置

**Files:** 新增 `app/src/config/turnstile_config.rs`、`migration/src/m20261014_000001_add_turnstile_secret.rs`；修改 `config.rs`、`config/default.toml`、`migration/src/lib.rs`、`entities/app_settings.rs`、`dto/settings.rs`、`dto/mod.rs`、`openapi.rs`、`settings_service.rs`、`settings_crypto.rs`、`settings_validation.rs` 与设置测试。

**Interfaces:** `CaptchaConfig.turnstile: TurnstileConfig`；配置包含 `enabled: bool, mode: String, site_key: String, secret_key: String, hostnames: Vec<String>`。`SettingsSnapshot.turnstile_secret_needs_reset: bool`。管理员 DTO `comment.captcha.turnstile` 包含公开设置及 `secret_configured`/`secret_needs_reset`；PATCH 同路径的 `secret_key` 只写。新增 `turnstile_secret_ciphertext: Option<String>`。

- [x] 写设置 API 行为测试，旧默认 GET 应包含关闭状态，PATCH 后应可保存三种模式并返回脱敏状态。

```rust
let patch = serde_json::from_value(serde_json::json!({
    "revision": 1, "comment": {"captcha": {"turnstile": {
        "enabled": true, "mode": "managed", "site_key": "site-key",
        "secret_key": "private-value", "hostnames": ["comments.example.com"]
    }}}
}));
assert!(patch.is_ok());
```

- [x] 运行 `cargo test --locked turnstile`，确认新增契约在实现前失败。
- [x] 新迁移只新增 nullable text 列，修改实体，旧 JSON 使用 `#[serde(default)]`，Secrets 使用 `skip_serializing` 和脱敏 Debug。

```rust
#[serde(default, skip_serializing)]
pub secret_key: String,
// 加解密使用 b"rustaline-turnstile-secret-v1"，SMTP 原标签不变。
```

- [x] 设置服务播种、读取、PATCH 原子 CAS 同时处理独立密文；正常编辑检查 enabled 必要字段，JWT 无法解密时允许加载和修复设置。
- [x] 规范化 hostname（去两端空格、小写、去重；拒绝 URL、端口、路径、通配符、空值、超过 253 字符；列表最多 100 项），模式三选一，密钥长度最多 4096，site_key 最多 255 且无空白。
- [x] 配置导入映射 `APP_COMMENT_TURNSTILE_ENABLED/MODE/SITE_KEY/SECRET_KEY/HOSTNAMES` 到嵌套路径；HOSTNAMES 为逗号分隔列表，旧已有数据库不被导入值覆盖。
- [x] 运行设置测试，包括空密钥保留、随机 nonce、与 SMTP 密文隔离、旧六次迁移后升级、JWT 轮换修复、revision 冲突、公共/管理员响应脱敏。
- [x] 保存独立提交，记录验证结果。

## Task 2: 固定 Siteverify 服务与评论门控

**Files:** 新增 `app/src/services/turnstile_service.rs` 和其测试模块；修改 workspace/app Cargo.toml、Cargo.lock、state.rs、main.rs、comment_service.rs、handlers/comment.rs、dto/comment.rs、dto/captcha.rs、handlers/captcha.rs、dto/mod.rs、openapi.rs、app/tests/api.rs 与现有 AppState 构造处。

**Interfaces:** `TurnstileVerifier::new() -> Result<Self, AppError>`，内部共享 reqwest Client；`verify(&self, config: &TurnstileConfig, token: Option<&str>, ip: &str) -> Result<(), AppError>`。`AppState.turnstile: TurnstileVerifier`。公共 config 增加 `turnstile: {enabled, mode, site_key}`。评论 DTO 增加 `turnstile_token: Option<String>`。

- [x] 写本地 HTTP 模拟服务测试，捕获 form 请求并返回明确的 Siteverify JSON。

```json
{"success":true,"hostname":"comments.example.com","action":"comment"}
```

- [x] 失败测试覆盖空/超长 token、success=false、错误 action/hostname、坏 JSON、非 2xx、网络和超时，关闭时零请求。
- [x] reqwest 在 workspace 统一版本（优先已锁定版本），仅 rustls 和 JSON；生产地址固定，测试地址只在 cfg(test) 构造器中替换，限制响应体大小，禁止重定向。
- [x] 评论服务在蜜罐、PoW、图形码之后调用 verify，使用同一个设置快照与 ClientIp，不新增提前消费接口；失败返回统一脱敏错误，不记录 token/secret。
- [x] 使用真实 router 检查开启后缺 token 拒绝、蜜罐假成功不外呼、不落库或生成通知任务，以及 config 响应不含秘密。
- [x] 运行 `cargo test --locked turnstile` 和 `cargo check --locked --all-targets`，更新全部构造处并提交。

## Task 3: SDK 三种交互和组件生命周期

**Files:** 修改 `static/sdk/rustaline.js`、`app/tests/sdk.cjs`；可新增独立 `app/tests/turnstile-sdk.cjs`，并将其纳入现有 SDK 测试命令使 CI 自动覆盖。

**Interfaces:** 公共 config `turnstile: {enabled:false|true, mode:"managed"|"invisible"|"click", site_key:"..."}`，缺少此字段等同关闭；POST 使用 `turnstile_token`；固定 widget action `comment`。

- [x] 先写动态开关与提交门控测试：开启后没有 token 不能 POST，失败保留草稿，关闭不加载脚本，旧服务端继续兼容，运行看到失败。
- [x] 页面共享脚本加载 Promise（有限超时、失败可重试）；每实例验证 Promise、widget ID、清理函数隔离；容器挂 `document.body` 防止 `_render` 清空。
- [x] 图形码收集与 PoW 完成后再运行 Turnstile，成功即 POST；API script 用 `https://challenges.cloudflare.com/turnstile/v0/api.js?render=explicit`。

```js
turnstile.render(container, {
  sitekey: config.site_key, action: 'comment', execution: 'execute',
  appearance: config.mode === 'managed' ? 'interaction-only' : 'always',
  'response-field': false, retry: 'never', 'refresh-expired': 'manual',
  callback: onToken, 'error-callback': onFailure,
  'expired-callback': onExpired, 'timeout-callback': onTimeout
});
```

- [x] invisible 使用真实 Invisible 组件；managed 在 before-interactive 时显示对话框；click 先显示验证按钮，点击后 execute，取消不执行。
- [x] 所有完成/取消/错误路径清理 widget、回调、定时器、对话框与焦点，取消和失败保持回复对象与草稿；重复点击只能一个提交。
- [x] 为错误、加载、取消、点击验证、隐私说明等新增同构中英字典；使用现有主题令牌与键盘焦点约束。
- [x] 回归覆盖 token 一次性、失败重试、多实例、脚本超时重试、动态关闭/切换、过期、销毁、取消与新草稿保护，运行 `node app/tests/sdk.cjs` 并提交。

## Task 4: 文件夹式后台 Turnstile 设置

**Files:** 修改 `static/admin/js/views/settings-runtime.js`、`static/admin/js/i18n.js`；仅必要时改 admin CSS。

**Interfaces:** 已有 `cfg.comment.captcha.turnstile` 与 PATCH 同路径。Key 输入框 `secret_key` 仅非空发出；配置状态使用 `secret_configured`、`secret_needs_reset`。

- [x] 在现有“验证码防护”内新增 fieldset：enabled、三种 mode、site_key、secret_key 密码输入、多行 hostnames。
- [x] 纳入现有 markDirty/readGroup/populate/setBusy；保持未保存输入，保存后密钥输入清空，409 保留编辑，不提供清除按钮。
- [x] 增加中英字典，解释 Invisible 与 Managed 密钥要求，“必须点击”由本项目按钮保证启动交互。
- [x] 浏览器核对加载设置→切换模式→修改域名→保存→重新读取，密码留空保留；检查浅/深主题、中/英文、桌面/390px 窄屏、无裁切和 console 错误。
- [x] 运行 SDK 回归、JS 语法检查并提交。

## Task 5: 文档、端到端验证与最终审查

**Files:** 修改 README.md、docs/guide/settings.md、AGENTS.md，更新本计划勾选状态。

- [x] 文档说明创建组件、域名列表应填写 SDK 所在站点、三种模式、旧数据默认关闭、组合防护、JWT 轮换修复、配置首次导入、CSP 与 Invisible 隐私政策要求。
- [x] 在本地临时 SQLite 测试环境使用官方测试密钥跑组件成功、强制交互、失败与后台设置流程，不调用管理员生产配置，不向外发邮件。
- [x] 运行 `cargo build --locked`、`cargo test --locked`、`cargo clippy --all-targets -- -D warnings`、`cargo fmt --check`、`node app/tests/sdk.cjs`、`npm run docs:build`。
- [x] 审查全部实现与设计一致性及日志/DTO 密钥保护、后台可修复性、失败不能绕过、SDK 不重复 POST；修复发现后复跑相关检查。
- [x] 保存最终实现提交，向用户报告完成项、验证证据和生产密钥未联调的范围，保持功能分支供查看。

## 验证证据与范围

测试覆盖 SQLite 旧库升级、独立随机密文、密钥保留、JWT 轮换、revision 冲突、模拟 Siteverify 成功／重放拒绝／异常，以及 SDK 三模式、回调时序、多实例和草稿保护。

最终验证包括构建、143 个 Rust 测试、Clippy、格式检查、SDK 回归、文档构建及 PostgreSQL／MySQL feature check；feature check 不等同于两种数据库的实库迁移验证。

官方 AA／BB 测试组件的前端成功／失败、组件清理和提交次数观察，与模拟服务及 SQLite 中验证的一次写入／重放拒绝分别记录。官方测试 Siteverify 返回的 hostname／action 与正式配置不同，因此不宣称完整生产 token 成功及重放联调。

官方 FF 强制交互测试遇到 `ERR_CONNECTION_CLOSED`；失败路径保持零 POST 和草稿保留，未宣称外部勾选流程成功。使用真实官方 AA 组件并触发交互回调后，稳定的父节点链仍能完成 token 获取。
