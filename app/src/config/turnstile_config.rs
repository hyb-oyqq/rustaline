use serde::{Deserialize, Serialize};

/// Turnstile 业务设置；密钥仅存在于启动配置和解密后的内存快照。
#[derive(Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct TurnstileConfig {
    pub enabled: bool,
    pub mode: String,
    pub site_key: String,
    #[serde(default, skip_serializing)]
    pub secret_key: String,
    pub hostnames: Vec<String>,
}

impl Default for TurnstileConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: "managed".into(),
            site_key: String::new(),
            secret_key: String::new(),
            hostnames: Vec::new(),
        }
    }
}

impl std::fmt::Debug for TurnstileConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TurnstileConfig")
            .field("enabled", &self.enabled)
            .field("mode", &self.mode)
            .field("site_key", &self.site_key)
            .field("secret_key", &"[REDACTED]")
            .field("hostnames", &self.hostnames)
            .finish()
    }
}
