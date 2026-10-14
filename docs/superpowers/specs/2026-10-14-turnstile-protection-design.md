# Cloudflare Turnstile 评论防护设计

日期：2026-10-14

状态：待用户确认。

## 目标与范围

管理员可以在后台配置 Cloudflare Turnstile，用于保护顶层评论和回复提交，并选择隐藏、自适应、必须点击三种交互方式。

沿用现有设置的数据库持久化、请求快照和 revision 冲突检查。管理员保存后，下一次评论提交读取新配置，无需重启。设置页面保持现有文件夹导航，在“验证码防护”中增加 Turnstile 分组，界面与 SDK 文案提供中英双语。

本次实现可由不同部署管理员自行配置的产品能力。管理员在 Cloudflare 控制台创建组件、配置评论所在网站的域名，再在本项目后台填写 Site Key 和 Secret Key。

## 方案比较

推荐把 Turnstile 作为第三种独立防护，与 PoW 和图形码沿用相同的开关规则：开启多项时全部通过，管理员也可关闭原有两项，仅使用 Turnstile。该方案保留已有部署行为，也支持管理员自行选择防护组合。

另一种方案是改成互斥的防护类型选择器，每次只能使用一种验证码。它能减少组合数量，但会改变既有 PoW 与图形码可以同时开启的语义，并需要额外迁移用户原有选择，因此本次采用独立开关方案。

## 三种交互方式

Cloudflare 官方组件类型是 Managed、Non-Interactive、Invisible；本项目的三种选择描述评论提交时的交互流程，两者不应混为一谈。

| 后台选项 | 配置值 | SDK 行为 | Cloudflare 组件要求 |
| --- | --- | --- | --- |
| 隐藏 | `invisible` | 提交时后台执行验证，不显示验证组件；验证失败在评论表单提示并保留草稿 | 使用 Invisible 类型的组件密钥，不能用 CSS 隐藏可能要求交互的 Managed 组件 |
| 自适应 | `managed` | 提交时执行验证，使用 `appearance: interaction-only`；Cloudflare 要求交互时显示验证区域 | 使用 Managed 类型的组件密钥 |
| 必须点击 | `click` | 提交后出现验证对话框，用户每次必须点击“点击验证”按钮，才开始 Turnstile 校验；成功后继续提交 | 使用 Managed 类型的组件密钥，点击按钮后显式执行验证 |

“必须点击”保证用户在本项目界面主动启动验证。Cloudflare 的 Managed 模式仍自行决定是否额外出现官方勾选框，没有每次强制勾选官方组件的配置项。仅设置 `appearance: always` 不能保证点击，因此不把它当作该需求的实现。

切换到隐藏模式时，后台说明 Site Key 对应的组件也应在 Cloudflare 控制台设置为 Invisible；切回自适应或必须点击时应使用 Managed 组件。SDK 不通过隐藏交互 iframe 来模拟隐藏模式。

部署文档同步说明官方脚本的 CSP 配置，并在隐藏模式的使用说明中提供 Cloudflare Turnstile 隐私政策链接，便于接入网站按官方 Invisible 模式要求补充其隐私说明。

## 设置、密钥与升级

在 `comment.captcha.turnstile` 中增加配置：

- `enabled`：默认 `false`。
- `mode`：`invisible`、`managed` 或 `click`，默认 `managed`。
- `site_key`：公开的组件标识。
- `hostnames`：允许提交的网站 hostname 列表；后台每行填写一个，服务端规范化并校验，匹配精确域名，不接收 URL、端口、路径或通配符。
- `secret_key`：仅用于首次导入和管理员写入，序列化业务 JSON 时省略。

Secret Key 使用 AES-256-GCM、随机 nonce 和从 JWT 派生的独立 Turnstile 密钥加密，保存到 `app_settings.turnstile_secret_ciphertext`。SMTP 现有密文格式及派生标签保持兼容。

新增迁移只增加可空密文列，不修改既有迁移文件或重建评论数据。旧设置 JSON 缺少 Turnstile 字段时通过 serde 默认值加载，默认关闭，原 PoW、图形码、邮件设置、revision 和管理员凭据保留。新安装可从 TOML/APP_* 首次导入，数据库已有设置仍优先。

管理员 GET 响应提供 `secret_configured` 与 `secret_needs_reset`，不返回 Secret Key 原文或密文。PATCH 中密钥省略或空字符串表示保留，非空表示替换；不新增清除已保存密钥的按钮。

首次启用需要有效 Site Key、Secret Key 及至少一个允许的 hostname。切换 JWT 后若密文无法解密，保持 Turnstile 防护开启并阻止受保护提交，后台提示重新填写密钥；后台登录、设置修改及其他已有业务仍可用，不能因解密失败自动放行评论。

## 服务端校验

公共 `GET /api/v1/captcha/config` 增加 `turnstile` 对象，仅公开 `enabled`、`mode` 和启用时需要的 `site_key`。不下发 Secret Key、密文或后台重设状态。

`POST /api/v1/comments` 增加可选 `turnstile_token`。关闭 Turnstile 时不要求该字段，旧客户端保持兼容；开启后缺少 token 的请求必须被拒绝。

新 `turnstile_service` 使用共享异步 HTTP 客户端向固定的 `https://challenges.cloudflare.com/turnstile/v0/siteverify` 发送表单请求：

- `secret` 为当前请求设置快照中的 Secret Key。
- `response` 为客户端 token，必须非空且不超过 2048 字符。
- `remoteip` 沿用服务端现有 ClientIp 推导结果。
- 前端固定设置 action 为 `comment`；服务端要求响应 `success == true`、`action == comment`，且 `hostname` 精确匹配管理员允许的网站域名。

使用 HTTPS、正常证书校验、有限连接超时和 10 秒总超时；禁止重定向到其他地址，不允许后台自定义 Siteverify URL。网络失败、非成功 HTTP、无效 JSON、配置不可用和校验失败均不能放行，错误经统一 AppError 脱敏返回，不记录密钥或 token。

顺序为现有评论限流 → 蜜罐 → PoW → 图形码 → Turnstile → 字段校验 → 评论与通知事务。蜜罐继续假成功，不向 Cloudflare 发请求，不入库或入队；前两种防护未通过时不产生外部验证请求。

Turnstile token 仅在评论提交接口校验一次，不增加提前消费 token 的预检接口。Cloudflare 负责五分钟有效期与单次消费；SDK 每次提交尝试获取新 token，不复用失败请求已经消费的 token，不自动重复 POST。

## SDK 与后台界面

SDK 保持单文件原生 JS，按当前请求刷新后的配置决定是否加载 Cloudflare 官方 `api.js?render=explicit`。脚本在页面中共享一次加载过程，加载失败可在后续重试；关闭防护不加载脚本。

每个 SDK 实例持有自己的组件 ID 与验证生命周期。组件容器放在不会被 `_render` 清空的位置，显式清理组件、回调、定时器和对话框，避免切换回复、刷新列表或多实例使用时串 token。验证期间防止重复提交；取消、失败、超时、脚本被拦截均保留正文、身份和回复对象，允许重新验证。

先完成图形码收集与 PoW 计算，再获取 Turnstile token 并立即提交，减少 token 等待过期的机会。自适应交互和必须点击对话框支持键盘、取消、焦点恢复、窄屏、明暗主题和中英文。验证成功只进入既有评论提交流程，沿用待审核提示、乐观插入和失败草稿恢复。

后台在原“验证码防护”文件夹增加一个独立 Turnstile 分组：开关、三种模式、Site Key、Secret Key 密码输入框及允许域名多行输入框，显示密钥配置状态与组件类型说明。密码框加载和保存后清空，留空保留已保存密钥，沿用未保存标记、统一保存和 409 冲突提示。

## 验证与验收

- 迁移与设置：旧版 SQLite 迁移后原设置不变、Turnstile 默认关闭；保存后即时生效，重启可恢复；密钥加密、不回显、空值保留、JWT 轮换与并发 revision 冲突符合设计。
- 服务端：使用本机模拟 Siteverify 测试成功、缺少/超长 token、失败或重复 token、action/hostname 错配、异常 HTTP、无效 JSON 与超时；检查关闭时零请求、蜜罐零请求，拒绝请求不产生评论或邮件任务。
- SDK：覆盖三种模式、必须点击之前不运行验证、脚本去重、失败后重试、过期/取消、组件清理、多实例 token 隔离、动态开关与草稿恢复。
- 浏览器：核对后台中英双语、主题与窄屏，使用 Cloudflare 官方测试密钥验证前端组件交互，并记录真实外部服务验证范围；自动化测试不依赖生产密钥。
- 完成后运行 `cargo build --locked`、`cargo test --locked`、`cargo clippy --all-targets -- -D warnings`、`cargo fmt --check`、SDK 回归与文档构建，并同步 OpenAPI、README、后台设置文档和 AGENTS.md。

Cloudflare 官方测试密钥可验证接入行为；生产域名、生产密钥和生产 token 的一次成功及重放验证，需要管理员配置其真实组件后在对应域名完成，未执行前不宣称生产联调完成。

## 官方资料

- 组件类型：https://developers.cloudflare.com/turnstile/concepts/widget/
- 前端选项：https://developers.cloudflare.com/turnstile/get-started/client-side-rendering/widget-configurations/
- 服务端 Siteverify：https://developers.cloudflare.com/turnstile/get-started/server-side-validation/
- 官方测试密钥：https://developers.cloudflare.com/turnstile/troubleshooting/testing/
