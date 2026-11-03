//! Independent adaptation of the copied sentinel-rs HTTP module.
//! Provenance: docs/source-provenance.md. Only GET transport is retained.
//! Each Client owns a fresh cookie jar. No implicit environment or system proxy.

use anyhow::{bail, Context, Result};
use futures_util::StreamExt;
use std::{sync::Arc, time::Duration};
use wreq::{
    cookie::Jar,
    header::{HeaderMap, HeaderName, HeaderValue},
};
use wreq_util::{Emulation, Profile};

pub const DEFAULT_EMULATION: Profile = Emulation::Chrome149;

pub fn parse_emulation(raw: &str) -> Option<Profile> {
    fn key(raw: &str) -> String {
        raw.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase()
    }
    let want = key(raw);
    Profile::VARIANTS
        .iter()
        .copied()
        .find(|p| key(&format!("{p:?}")) == want)
}

pub fn normalize_proxy_url(raw: &str) -> Result<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        bail!("empty_proxy")
    }
    if raw.len() > 4096 || raw.chars().any(|c| c.is_whitespace() || c.is_control()) {
        bail!("invalid_proxy_url")
    }
    let url = if raw.contains("://") {
        raw.to_owned()
    } else {
        format!("http://{raw}")
    };
    let uri: wreq::Uri = url
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid_proxy_url"))?;
    if uri.host().unwrap_or_default().is_empty()
        || !matches!(uri.path(), "" | "/")
        || uri.query().is_some()
        || url.contains('#')
        || !matches!(
            uri.scheme_str(),
            Some("http" | "https" | "socks5" | "socks5h")
        )
    {
        bail!("invalid_proxy_url")
    }
    wreq::Proxy::all(url.as_str()).map_err(|_| anyhow::anyhow!("invalid_proxy_url"))?;
    Ok(url)
}

#[derive(Clone)]
pub struct RequestSpec {
    pub label: &'static str,
    pub url: String,
    pub headers: Vec<(String, String)>,
}
impl std::fmt::Debug for RequestSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RequestSpec")
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}
impl RequestSpec {
    pub fn get(label: &'static str, url: impl Into<String>) -> Self {
        Self {
            label,
            url: url.into(),
            headers: vec![],
        }
    }
    pub fn with_headers(mut self, headers: Vec<(String, String)>) -> Self {
        self.headers = headers;
        self
    }
}

#[derive(Clone, Default)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}
impl std::fmt::Debug for HttpResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpResponse")
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}
impl HttpResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

pub struct Client {
    inner: wreq::Client,
}
impl Client {
    pub fn build(proxy: Option<&str>, profile: Profile) -> Result<Self> {
        let mut builder = wreq::Client::builder()
            .emulation(profile)
            .cookie_provider(Arc::new(Jar::default()))
            .timeout(Duration::from_secs(20))
            .retry(wreq::retry::Policy::never())
            .redirect(wreq::redirect::Policy::none())
            .no_proxy();
        // Keep the profile's own User-Agent instead of replacing it with an empty string.
        if let Some(raw) = proxy {
            let proxy = wreq::Proxy::all(normalize_proxy_url(raw)?)
                .map_err(|_| anyhow::anyhow!("invalid_proxy"))?;
            builder = builder.proxy(proxy);
        }
        Ok(Self {
            inner: builder
                .build()
                .map_err(|_| anyhow::anyhow!("http_client_initialization_failed"))?,
        })
    }
    /// Exactly one attempt; retry budgets belong exclusively to the query layer.
    pub async fn execute(&self, spec: &RequestSpec) -> Result<HttpResponse> {
        let mut headers = HeaderMap::new();
        for (name, value) in &spec.headers {
            let name = HeaderName::from_bytes(name.as_bytes()).context("invalid_header_name")?;
            let value = HeaderValue::from_str(value)
                .map_err(|_| anyhow::anyhow!("invalid_header_value"))?;
            headers.insert(name, value);
        }
        let resp = self
            .inner
            .get(&spec.url)
            .headers(headers)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("http_transport_error"))?;
        let status = resp.status().as_u16();
        // Do not retain cookies or arbitrary upstream headers in response DTOs.
        let headers = resp
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .map(|v| vec![("retry-after".to_owned(), v.to_owned())])
            .unwrap_or_default();
        let body = if status == 200 {
            if resp.content_length().is_some_and(|n| n > 262144) {
                bail!("response_too_large")
            }
            let mut stream = resp.bytes_stream();
            let mut bytes = Vec::new();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| anyhow::anyhow!("http_body_error"))?;
                if bytes.len().saturating_add(chunk.len()) > 262144 {
                    bail!("response_too_large")
                }
                bytes.extend_from_slice(&chunk);
            }
            String::from_utf8(bytes).map_err(|_| anyhow::anyhow!("invalid_response_encoding"))?
        } else {
            String::new()
        };
        tracing::debug!(label = spec.label, status, "query_response");
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }
}
