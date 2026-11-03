use anyhow::{bail, Context, Result};
use std::{net::SocketAddr, path::PathBuf, time::Duration};

#[derive(Clone)]
pub struct Config {
    pub db_path: PathBuf,
    pub listen: SocketAddr,
    pub proxy: Option<String>,
    pub mail_map_file: PathBuf,
    pub mail_api_token: Option<String>,
    pub mail_resolve_url: Option<String>,
    pub mail_resolve_token: Option<String>,
    pub otp_max_wait: Duration,
    pub otp_poll_interval: Duration,
    pub admin_token_file: PathBuf,
    pub log: String,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let get = |key: &str| lookup(key).filter(|v| !v.trim().is_empty());
        let seconds = |key, default: u64| -> Result<Duration> {
            let value = get(key)
                .map(|v| v.parse::<u64>())
                .transpose()
                .with_context(|| format!("{key} 必须为正整数"))?
                .unwrap_or(default);
            if value == 0 {
                bail!("{key} 必须大于零");
            }
            Ok(Duration::from_secs(value))
        };
        let listen: SocketAddr = get("LOGIN_LISTEN")
            .unwrap_or_else(|| "127.0.0.1:8788".into())
            .parse()
            .context("LOGIN_LISTEN 无效")?;
        if !listen.ip().is_loopback() || listen.port() == 0 {
            bail!("LOGIN_LISTEN 必须为非零端口的回环地址");
        }
        let proxy = get("LOGIN_PROXY")
            .map(|v| sentinel_rs::flow::http::normalize_proxy_url(v.trim()))
            .transpose()
            .map_err(|_| anyhow::anyhow!("LOGIN_PROXY 格式无效"))?;
        let log = get("LOGIN_LOG").unwrap_or_else(|| "info".into());
        if !["error", "warn", "info", "debug", "trace"].contains(&log.as_str()) {
            bail!("LOGIN_LOG 必须是日志级别");
        }
        Ok(Self {
            db_path: get("LOGIN_DB_PATH")
                .context("必须指定 LOGIN_DB_PATH")?
                .into(),
            listen,
            proxy,
            mail_map_file: get("LOGIN_MAIL_MAP_FILE")
                .unwrap_or_else(|| "data/mail-map.json".into())
                .into(),
            mail_api_token: get("LOGIN_MAIL_API_TOKEN"),
            mail_resolve_url: get("LOGIN_MAIL_RESOLVE_URL"),
            mail_resolve_token: get("LOGIN_MAIL_RESOLVE_TOKEN"),
            otp_max_wait: seconds("LOGIN_OTP_MAX_WAIT", 180)?,
            otp_poll_interval: seconds("LOGIN_OTP_POLL_INTERVAL", 3)?,
            admin_token_file: get("LOGIN_ADMIN_TOKEN_FILE")
                .unwrap_or_else(|| "data/admin.token".into())
                .into(),
            log,
        })
    }
}
