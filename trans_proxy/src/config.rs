use std::{env, fs, path::PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct Config {
    #[serde(default = "default_http_bind")]
    pub http_bind: String,
    #[serde(default = "default_https_bind")]
    pub https_bind: String,
    #[serde(default)]
    pub upstream_http_proxy: UpstreamHttpProxyConfig,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct UpstreamHttpProxyConfig {
    #[serde(default)]
    pub enabled: bool,
    pub address: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            http_bind: default_http_bind(),
            https_bind: default_https_bind(),
            upstream_http_proxy: UpstreamHttpProxyConfig::default(),
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = env::var("TRANS_PROXY_CONFIG").ok().map(PathBuf::from);
        let mut config = if let Some(path) = path {
            load_from_path(path)?
        } else {
            load_default_file_if_present()?
        };

        apply_env_overrides(&mut config);
        normalize(&mut config);
        Ok(config)
    }
}

fn load_default_file_if_present() -> Result<Config> {
    let path = PathBuf::from("trans_proxy.toml");
    if path.exists() {
        load_from_path(path)
    } else {
        Ok(Config::default())
    }
}

fn load_from_path(path: PathBuf) -> Result<Config> {
    let content = fs::read_to_string(&path)
        .with_context(|| format!("failed to read config file {}", path.display()))?;
    toml::from_str(&content)
        .with_context(|| format!("failed to parse config file {}", path.display()))
}

fn apply_env_overrides(config: &mut Config) {
    if let Ok(value) = env::var("HTTP_BIND") {
        config.http_bind = value;
    }
    if let Ok(value) = env::var("HTTPS_BIND") {
        config.https_bind = value;
    }
    if let Ok(value) = env::var("UPSTREAM_HTTP_PROXY_ENABLED") {
        if let Some(parsed) = parse_bool(&value) {
            config.upstream_http_proxy.enabled = parsed;
        }
    }
    if let Ok(value) = env::var("UPSTREAM_HTTP_PROXY_ADDR") {
        config.upstream_http_proxy.address = Some(value);
    }
    if let Ok(value) = env::var("UPSTREAM_HTTP_PROXY_USERNAME") {
        config.upstream_http_proxy.username = Some(value);
    }
    if let Ok(value) = env::var("UPSTREAM_HTTP_PROXY_PASSWORD") {
        config.upstream_http_proxy.password = Some(value);
    }
}

fn normalize(config: &mut Config) {
    config.upstream_http_proxy.address = normalize_optional(config.upstream_http_proxy.address.take());
    config.upstream_http_proxy.username = normalize_optional(config.upstream_http_proxy.username.take());
    config.upstream_http_proxy.password = normalize_optional(config.upstream_http_proxy.password.take());
}

fn normalize_optional(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn default_http_bind() -> String {
    "0.0.0.0:80".to_string()
}

fn default_https_bind() -> String {
    "0.0.0.0:443".to_string()
}
