use super::{mapping, resolver, Mailbox};
use crate::{config::Config, BoxFuture};
use anyhow::{bail, Result};
use sentinel_rs::flow::otp;
use std::{collections::HashSet, path::PathBuf, time::Duration};

pub struct MailTicket {
    pub url: String,
    pub baseline: HashSet<String>,
    pub issued_after: i64,
}

pub struct FileMailbox {
    map_file: PathBuf,
    token: Option<String>,
    resolve_url: Option<String>,
    resolve_token: Option<String>,
    client: wreq::Client,
    max_wait: Duration,
    interval: Duration,
}

impl FileMailbox {
    pub fn new(config: &Config) -> Result<Self> {
        Ok(Self {
            map_file: config.mail_map_file.clone(),
            token: config.mail_api_token.clone(),
            resolve_url: config.mail_resolve_url.clone(),
            resolve_token: config.mail_resolve_token.clone(),
            client: wreq::Client::builder()
                .user_agent(concat!("sentinel-relogin/", env!("CARGO_PKG_VERSION")))
                .no_proxy()
                .redirect(wreq::redirect::Policy::none())
                .timeout(Duration::from_secs(15))
                .build()
                .map_err(|_| anyhow::anyhow!("无法创建独立邮箱读取器"))?,
            max_wait: config.otp_max_wait,
            interval: config.otp_poll_interval,
        })
    }

    async fn fetch(&self, url: &str) -> Result<String> {
        let mut request = self.client.get(url);
        if let Some(token) = self.token.as_ref().filter(|_| self.resolve_url.is_none()) {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let response = request
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("重登邮箱取码请求失败"))?;
        if !response.status().is_success() {
            if response.status().as_u16() == 410 {
                bail!("重登邮箱取码链接已过期，请重新发起重登");
            }
            bail!("重登邮箱取码返回 HTTP {}", response.status().as_u16());
        }
        response
            .text()
            .await
            .map_err(|_| anyhow::anyhow!("无法读取取码响应"))
    }
}

impl Mailbox for FileMailbox {
    fn prepare<'a>(&'a self, email: &'a str) -> BoxFuture<'a, Result<MailTicket>> {
        Box::pin(async move {
            let url = match &self.resolve_url {
                Some(endpoint) => {
                    resolver::resolve(&self.client, endpoint, self.resolve_token.as_deref(), email)
                        .await?
                }
                None => mapping::resolve(&self.map_file, email)?,
            };
            let body = self.fetch(&url).await?;
            Ok(MailTicket {
                url,
                baseline: codes(&body),
                issued_after: crate::now(),
            })
        })
    }

    fn poll<'a>(&'a self, ticket: &'a MailTicket) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let deadline = tokio::time::Instant::now() + self.max_wait;
            loop {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    bail!("等待重登邮箱验证码超时");
                }
                let body = tokio::time::timeout(remaining, self.fetch(&ticket.url))
                    .await
                    .map_err(|_| anyhow::anyhow!("等待重登邮箱验证码超时"))??;
                if let Some(code) = fresh_code(&body, ticket.issued_after, &ticket.baseline) {
                    return Ok(code);
                }
                tokio::time::sleep(
                    self.interval
                        .min(deadline.saturating_duration_since(tokio::time::Instant::now())),
                )
                .await;
            }
        })
    }
}

fn single_code(body: &str) -> Option<String> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
        for node in [&value, &value["data"]] {
            for key in ["code", "otp"] {
                if let Some(raw) = node[key].as_str() {
                    if let Some(code) = otp::extract_otp(raw) {
                        return Some(code);
                    }
                }
            }
        }
    }
    let raw = body.trim();
    (raw.len() == 6 && raw.bytes().all(|b| b.is_ascii_digit())).then(|| raw.to_owned())
}

pub fn codes(body: &str) -> HashSet<String> {
    let mut codes: HashSet<_> = otp::parse_cards(body)
        .iter()
        .filter_map(|c| c.otp())
        .collect();
    if let Some(code) = single_code(body) {
        codes.insert(code);
    }
    codes
}

pub fn fresh_code(body: &str, after: i64, baseline: &HashSet<String>) -> Option<String> {
    if !otp::parse_cards(body).is_empty() {
        return otp::select_fresh_otp(body, Some(after), baseline);
    }
    single_code(body).filter(|code| !baseline.contains(code))
}
