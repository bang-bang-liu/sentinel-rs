use crate::BoxFuture;
use anyhow::{bail, Result};
use sentinel_rs::flow::{
    auth::{self, AuthRequestBuilder},
    authorize,
    http::{self, HttpResponse, RequestSpec},
    SentinelToken, SentinelTokenSource,
};
use serde_json::Value;
use std::sync::Arc;

pub trait Transport: Send + Sync {
    fn execute<'a>(&'a self, request: &'a RequestSpec) -> BoxFuture<'a, Result<HttpResponse>>;
    fn cookie_header_for(&self, url: &str) -> String;
}

impl Transport for http::Client {
    fn execute<'a>(&'a self, request: &'a RequestSpec) -> BoxFuture<'a, Result<HttpResponse>> {
        Box::pin(self.execute(request))
    }
    fn cookie_header_for(&self, url: &str) -> String {
        self.cookie_header_for(url)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Identify,
    Password,
    Email,
    Totp,
    Continue,
    Unknown,
}

#[derive(Clone, Debug)]
pub struct Step {
    pub action: Action,
    pub continue_url: String,
    pub factor_id: String,
}

fn url_path(raw: &str) -> String {
    url::Url::parse(&auth::absolute_url(raw, auth::AUTH_BASE))
        .map(|u| u.path().to_owned())
        .unwrap_or_default()
}

pub fn trusted_url(raw: &str) -> Result<String> {
    let absolute = auth::absolute_url(raw, auth::AUTH_BASE);
    let url = url::Url::parse(&absolute).map_err(|_| anyhow::anyhow!("登录继续地址无效"))?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("auth.openai.com" | "chatgpt.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port_or_known_default() != Some(443)
    {
        bail!("登录继续地址不是受支持的认证站点");
    }
    Ok(url.into())
}

fn action(page: &str, next: &str) -> Action {
    let path = url_path(next);
    if path.starts_with("/create-account")
        || path == "/about-you"
        || page.starts_with("create_account")
    {
        return Action::Unknown;
    }
    if page == "login_password" || path == "/log-in/password" {
        return Action::Password;
    }
    if page == "email_otp_verification" || path == "/email-verification" {
        return Action::Email;
    }
    if page == "mfa_challenge" || path.starts_with("/mfa-challenge") {
        return Action::Totp;
    }
    if matches!(page, "log_in" | "login" | "login_identifier") || path == "/log-in" {
        return Action::Identify;
    }
    if matches!(
        page,
        "workspace" | "workspace_selection" | "select_workspace"
    ) {
        return Action::Continue;
    }
    if !next.is_empty()
        && matches!(
            page,
            "" | "external_url"
                | "url"
                | "redirect"
                | "callback"
                | "oauth_authorize"
                | "consent"
                | "workspace"
                | "workspace_selection"
                | "select_workspace"
                | "login_success"
                | "login_finalizer"
        )
    {
        return Action::Continue;
    }
    Action::Unknown
}

fn factor_from_url(raw: &str) -> Option<String> {
    let path = url_path(raw);
    path.strip_prefix("/mfa-challenge/")
        .and_then(|s| s.split('/').next())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn factor(node: &Value) -> Option<String> {
    if let Some(obj) = node.as_object() {
        for key in ["factor_id", "challenge_id"] {
            if let Some(id) = obj
                .get(key)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            {
                return Some(id.to_owned());
            }
        }
        for key in ["url", "continue_url"] {
            if let Some(id) = obj
                .get(key)
                .and_then(Value::as_str)
                .and_then(factor_from_url)
            {
                return Some(id);
            }
        }
        for key in ["mfa_challenge_factors", "mfa_factors", "factors"] {
            if let Some(items) = obj.get(key).and_then(Value::as_array) {
                for item in items {
                    let kind = item
                        .get("type")
                        .or_else(|| item.get("factor_type"))
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    if matches!(kind, "totp" | "otp_totp" | "") {
                        if let Some(id) = item
                            .get("id")
                            .or_else(|| item.get("factor_id"))
                            .and_then(Value::as_str)
                            .filter(|s| !s.is_empty())
                        {
                            return Some(id.to_owned());
                        }
                    }
                }
            }
        }
        for key in ["page", "payload", "data", "challenge"] {
            if let Some(id) = obj.get(key).and_then(factor) {
                return Some(id);
            }
        }
    }
    None
}

pub fn parse_step(body: &str) -> Result<Step> {
    let value: Value =
        serde_json::from_str(body).map_err(|_| anyhow::anyhow!("登录响应不是 JSON"))?;
    let node = value
        .get("data")
        .filter(|v| v.is_object())
        .unwrap_or(&value);
    let page = &node["page"];
    let kind = page["type"]
        .as_str()
        .or_else(|| node["page_type"].as_str())
        .unwrap_or("");
    let next = [
        &node["continue_url"],
        &page["continue_url"],
        &page["url"],
        &page["payload"]["continue_url"],
        &node["url"],
    ]
    .iter()
    .find_map(|v| v.as_str().filter(|s| !s.is_empty()))
    .unwrap_or("");
    Ok(Step {
        action: action(kind, next),
        continue_url: next.to_owned(),
        factor_id: factor(node).unwrap_or_default(),
    })
}

pub async fn request(transport: &dyn Transport, spec: &RequestSpec) -> Result<HttpResponse> {
    transport
        .execute(spec)
        .await
        .map_err(|_| anyhow::anyhow!("网络请求失败（{}）", spec.label))
}

pub fn success(response: &HttpResponse, label: &str) -> Result<()> {
    if response.is_success() {
        return Ok(());
    }
    // Only expose fixed descriptions; remote bodies may contain credentials or mailbox URLs.
    let value: Value = serde_json::from_str(&response.body).unwrap_or(Value::Null);
    let reason = match value["error"]["code"]
        .as_str()
        .or_else(|| value["code"].as_str())
    {
        Some("incorrect_password" | "invalid_password" | "invalid_username_or_password") => {
            "，密码不正确"
        }
        Some("wrong_email_otp_code" | "invalid_email_otp") => "，邮箱验证码不正确或已失效",
        Some("invalid_totp" | "invalid_mfa_code" | "incorrect_mfa_code") => "，TOTP 验证失败",
        Some("invalid_state") => "，登录会话状态已失效",
        Some("account_deactivated" | "account_disabled") => "，账号不可用",
        _ => "",
    };
    bail!("{label}返回 HTTP {}{reason}", response.status)
}

pub async fn step_request(transport: &dyn Transport, spec: &RequestSpec) -> Result<Step> {
    let response = request(transport, spec).await?;
    if (300..400).contains(&response.status) {
        let next = response.header("location").unwrap_or("");
        return Ok(Step {
            action: action("", next),
            continue_url: next.into(),
            factor_id: factor_from_url(next).unwrap_or_default(),
        });
    }
    success(&response, spec.label)?;
    parse_step(&response.body)
}

pub async fn token(
    source: Arc<dyn SentinelTokenSource>,
    flow: &str,
    device_id: &str,
) -> Result<SentinelToken> {
    let flow = flow.to_owned();
    let device_id = device_id.to_owned();
    tokio::task::spawn_blocking(move || source.token(&flow, &device_id))
        .await
        .map_err(|_| anyhow::anyhow!("认证请求准备任务失败"))?
        .map_err(|_| anyhow::anyhow!("无法准备认证请求，请检查账号代理或稍后重试"))
}

fn login_hint(raw: &str) -> Result<String> {
    let mut url =
        url::Url::parse(&trusted_url(raw)?).map_err(|_| anyhow::anyhow!("authorize 地址无效"))?;
    let pairs: Vec<_> = url
        .query_pairs()
        .filter(|(key, _)| key != "screen_hint")
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    url.set_query(None);
    url.query_pairs_mut()
        .extend_pairs(pairs)
        .append_pair("screen_hint", "login");
    Ok(url.into())
}

pub async fn initialize(
    transport: &dyn Transport,
    builder: &AuthRequestBuilder,
    email: &str,
) -> Result<(String, Step)> {
    let mut warmup = None;
    for attempt in 1..=authorize::WARMUP_ATTEMPTS {
        if let Ok(response) = request(transport, &builder.warmup_spec()).await {
            if response.is_success() {
                if let Ok(csrf) = auth::parse_csrf_token(&response.body) {
                    let did = auth::cookie_value(
                        &transport.cookie_header_for(auth::CHATGPT_BASE),
                        "oai-did",
                    )
                    .unwrap_or_default();
                    if !did.is_empty() {
                        warmup = Some((csrf, did));
                        break;
                    }
                }
            }
        }
        if attempt < authorize::WARMUP_ATTEMPTS {
            tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
        }
    }
    let (csrf, did) =
        warmup.ok_or_else(|| anyhow::anyhow!("无法建立登录会话，请检查账号代理或稍后重试"))?;
    let mut spec =
        builder.signin_openai_spec(email, &csrf, &did, builder.auth_session_logging_id());
    spec.url = login_hint(&spec.url)?;
    let response = request(transport, &spec).await?;
    success(&response, "获取登录入口")?;
    let raw = auth::parse_signin_url(&response.body)
        .map_err(|_| anyhow::anyhow!("登录入口未返回 authorize 地址"))?;
    let mut current = login_hint(&auth::ensure_authorize_context(
        &raw,
        email,
        &did,
        builder.auth_session_logging_id(),
    ))?;
    for _ in 0..authorize::MAX_AUTHORIZE_HOPS {
        let response = request(
            transport,
            &builder.authorize_init_spec(&trusted_url(&current)?),
        )
        .await?;
        if (300..400).contains(&response.status) {
            current = trusted_url(
                response
                    .header("location")
                    .ok_or_else(|| anyhow::anyhow!("登录入口重定向缺少地址"))?,
            )?;
            continue;
        }
        success(&response, "初始化登录状态")?;
        let device_id =
            auth::cookie_value(&transport.cookie_header_for(auth::CHATGPT_BASE), "oai-did")
                .or_else(|| {
                    auth::cookie_value(&transport.cookie_header_for(auth::AUTH_BASE), "oai-did")
                })
                .unwrap_or(did);
        let step = Step {
            action: action("", &current),
            factor_id: factor_from_url(&current).unwrap_or_default(),
            continue_url: current,
        };
        builder.new_document_navigation();
        return Ok((device_id, step));
    }
    bail!("登录入口重定向次数超限")
}

async fn select_workspace(
    transport: &dyn Transport,
    builder: &AuthRequestBuilder,
    current: &str,
    html: &str,
) -> Result<String> {
    let id = auth::workspace_id_from_url(current)
        .or_else(|| {
            auth::workspace_id_from_cookie_header(&transport.cookie_header_for(auth::AUTH_BASE))
        })
        .or_else(|| auth::workspace_id_from_html(html))
        .ok_or_else(|| anyhow::anyhow!("登录未返回可用的 workspace"))?;
    let response = request(transport, &builder.select_workspace(&id)).await?;
    success(&response, "选择 workspace")?;
    let parsed = auth::parse_workspace_select(response.status, &response.body)
        .map_err(|_| anyhow::anyhow!("workspace 响应无效"))?;
    trusted_url(&parsed.continue_url)
}

pub async fn finish(
    transport: &dyn Transport,
    builder: &AuthRequestBuilder,
    start: &str,
) -> Result<String> {
    let mut current = if start.is_empty() {
        select_workspace(transport, builder, "", "").await?
    } else {
        trusted_url(start)?
    };
    let mut referer = format!("{}/", auth::AUTH_BASE);
    let mut callback_seen = false;
    for _ in 0..12 {
        let is_callback = auth::is_callback_url(&current);
        if is_callback && auth::callback_code(&current).is_none() {
            bail!("登录回调未返回授权码");
        }
        let response = request(transport, &builder.navigation_from(&current, &referer)).await?;
        if is_callback {
            if response.status >= 400 {
                success(&response, "完成登录回调")?;
            }
            callback_seen = true;
        }
        if (300..400).contains(&response.status) {
            let next = response
                .header("location")
                .ok_or_else(|| anyhow::anyhow!("登录重定向缺少地址"))?;
            // Callback already sets the session cookie. No unrelated home-page navigation is needed.
            if callback_seen {
                break;
            }
            referer = current;
            current = trusted_url(next)?;
            continue;
        }
        success(&response, "完成登录跳转")?;
        if is_callback {
            break;
        }
        if auth::is_workspace_page(&current) {
            current = select_workspace(transport, builder, &current, &response.body).await?;
            continue;
        }
        // Some finalizer responses return a JSON continuation rather than HTTP Location.
        if let Ok(step) = parse_step(&response.body) {
            if step.action == Action::Continue && !step.continue_url.is_empty() {
                referer = current;
                current = trusted_url(&step.continue_url)?;
                continue;
            }
        }
        break;
    }
    if !callback_seen {
        bail!("未完成登录回调，账号未更新");
    }
    session(transport, builder).await
}

pub async fn session(transport: &dyn Transport, builder: &AuthRequestBuilder) -> Result<String> {
    let response = request(transport, &builder.chat_session()).await?;
    success(&response, "获取 Access Token")?;
    let session = auth::parse_chat_session(&response.body)
        .map_err(|_| anyhow::anyhow!("session 响应无效"))?;
    if session.access_token.trim().is_empty() {
        bail!("session 未返回 Access Token，账号未更新");
    }
    Ok(session.access_token)
}

pub fn cookies(transport: &dyn Transport) -> String {
    let mut parts: Vec<(String, String)> = Vec::new();
    for url in [auth::AUTH_BASE, auth::CHATGPT_BASE] {
        for part in transport.cookie_header_for(url).split(';') {
            let part = part.trim();
            if let Some((name, _)) = part.split_once('=') {
                if !parts
                    .iter()
                    .any(|(n, _)| n.eq_ignore_ascii_case(name.trim()))
                {
                    parts.push((name.trim().to_owned(), part.to_owned()));
                }
            }
        }
    }
    parts
        .into_iter()
        .map(|(_, value)| value)
        .collect::<Vec<_>>()
        .join("; ")
}
