use super::config::Config;
use crate::{
    domain::{
        campaign::{classify, Outcome},
        model::now,
    },
    infrastructure::http::{parse_emulation, Client, HttpResponse, RequestSpec, DEFAULT_EMULATION},
};
use std::time::{Duration, SystemTime};

pub fn client(config: &Config) -> anyhow::Result<Client> {
    let profile = config
        .emulation
        .as_deref()
        .and_then(parse_emulation)
        .unwrap_or(DEFAULT_EMULATION);
    Client::build(config.proxy.as_deref(), profile)
}

pub fn retry_after(raw: Option<&str>, current: SystemTime) -> Option<u64> {
    let raw = raw?.trim();
    if let Ok(n) = raw.parse::<u64>() {
        return Some(n);
    }
    httpdate::parse_http_date(raw)
        .ok()
        .map(|date| date.duration_since(current).unwrap_or_default().as_secs())
}

pub struct Probe {
    pub outcome: Outcome,
    pub stop_until: Option<i64>,
}

pub async fn probe(client: &Client, token: &str, coupon: &str) -> Probe {
    let url=format!("https://chatgpt.com/backend-api/promo_campaign/check_coupon?coupon={coupon}&is_coupon_from_query_param=true");
    let spec = RequestSpec::get("offers.check", url).with_headers(vec![
        ("Authorization".into(), format!("Bearer {token}")),
        ("Accept".into(), "application/json".into()),
        ("Origin".into(), "https://chatgpt.com".into()),
        ("Referer".into(), "https://chatgpt.com/".into()),
    ]);
    probe_with(coupon, || client.execute(&spec)).await
}

// Injected transport supports deterministic offline retry tests; production URL stays fixed.
pub async fn probe_with<F, Fut>(coupon: &str, mut send: F) -> Probe
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<HttpResponse>>,
{
    for attempt in 1..=3u32 {
        let reply = tokio::time::timeout(Duration::from_secs(20), send()).await;
        let mut outcome;
        let mut delay = 1u64 << (attempt - 1);
        let mut cooldown = None;
        let retryable;
        match reply {
            Ok(Ok(response)) => {
                outcome = classify(coupon, response.status, &response.body);
                retryable = response.status == 429 || response.status >= 500;
                if response.status == 429 {
                    delay = retry_after(response.header("retry-after"), SystemTime::now())
                        .unwrap_or(5u64 << (attempt - 1))
                        .max(1);
                    if delay > 60 || attempt == 3 {
                        cooldown =
                            Some(now().saturating_add(delay.max(60).min(i64::MAX as u64) as i64));
                    }
                }
            }
            Ok(Err(_)) => {
                outcome = Outcome::unknown("network_error", None);
                retryable = true;
            }
            Err(_) => {
                outcome = Outcome::unknown("timeout", None);
                retryable = true;
            }
        }
        outcome.attempts = attempt;
        if !retryable || attempt == 3 || cooldown.is_some() {
            return Probe {
                outcome,
                stop_until: cooldown,
            };
        }
        tokio::time::sleep(Duration::from_secs(delay)).await;
    }
    unreachable!()
}
