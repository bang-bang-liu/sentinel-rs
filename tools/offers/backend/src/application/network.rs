use super::config::Config;
use crate::infrastructure::{http::normalize_proxy_url, results::Results};
use anyhow::Result;
use serde::Serialize;

#[derive(Serialize)]
pub struct NetworkSettings {
    pub mode: &'static str,
    pub display: Option<String>,
    pub source: &'static str,
}

/// A task receives one owned snapshot. Later dashboard edits affect future tasks only.
pub fn snapshot(config: &Config, results: &Results) -> Result<(Config, &'static str)> {
    let mut effective = config.clone();
    if config.command == "check" && config.proxy_explicit {
        return Ok((effective, "command_line"));
    }
    if let Some(saved) = results.query_proxy()? {
        effective.proxy = if saved.is_empty() { None } else { Some(saved) };
        return Ok((effective, "dashboard"));
    }
    Ok((effective, "startup"))
}

pub fn settings(config: &Config, results: &Results) -> Result<NetworkSettings> {
    let (effective, source) = snapshot(config, results)?;
    let display = effective.proxy.as_deref().map(redacted_proxy).transpose()?;
    Ok(NetworkSettings {
        mode: if display.is_some() { "proxy" } else { "direct" },
        display,
        source,
    })
}

fn redacted_proxy(raw: &str) -> Result<String> {
    let normalized = normalize_proxy_url(raw)?;
    let uri: wreq::Uri = normalized.parse()?;
    let authority = uri.authority().expect("validated proxy authority").as_str();
    let endpoint = match authority.rsplit_once('@') {
        Some((_, host)) => format!("***@{host}"),
        None => authority.to_owned(),
    };
    Ok(format!("{}://{endpoint}", uri.scheme_str().unwrap()))
}
