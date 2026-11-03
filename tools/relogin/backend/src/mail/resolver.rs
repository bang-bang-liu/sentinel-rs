use anyhow::{bail, Context, Result};
use sentinel_rs::flow::otp;
use serde::Deserialize;
use url::Url;

#[derive(Deserialize)]
struct ResolvedMailbox {
    email: String,
    code_url: String,
    expires_at: String,
}

/// 仅在邮箱挑战时解析配置，允许 HTTPS 或本机 HTTP 联调。
fn parse_url(raw: &str) -> Result<Url> {
    let url = Url::parse(raw).map_err(|_| anyhow::anyhow!("重登邮箱接口 URL 格式无效"))?;
    let loopback = url.host_str().is_some_and(|host| {
        host == "localhost"
            || host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    if !(url.scheme() == "https" || (url.scheme() == "http" && loopback))
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        bail!("重登邮箱接口必须使用 HTTPS 或回环 HTTP，且不含用户凭据或片段");
    }
    Ok(url)
}

/// 为本轮挑战请求新链接；全局密钥只发送给此签发接口。
pub async fn resolve(
    client: &wreq::Client,
    endpoint: &str,
    token: Option<&str>,
    email: &str,
) -> Result<String> {
    let endpoint = parse_url(endpoint)?;
    let token = token
        .filter(|value| !value.trim().is_empty())
        .context("API 模式必须配置 LOGIN_MAIL_RESOLVE_TOKEN")?;
    let response = client
        .post(endpoint.as_str())
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(serde_json::to_vec(&serde_json::json!({ "email": email }))?)
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("自动获取取码地址请求失败"))?;
    if response.status().as_u16() != 201 {
        let reason = match response.status().as_u16() {
            401 => "重登邮箱接口密钥无效",
            404 => "该邮箱没有可用的自动取码地址",
            429 => "重登邮箱接口请求过于频繁",
            503 => "重登邮箱接口未就绪",
            _ => "自动获取取码地址失败",
        };
        bail!("{reason}（HTTP {}）", response.status().as_u16());
    }
    let body = response
        .text()
        .await
        .map_err(|_| anyhow::anyhow!("无法读取自动取码地址响应"))?;
    let resolved: ResolvedMailbox =
        serde_json::from_str(&body).map_err(|_| anyhow::anyhow!("自动取码地址响应格式无效"))?;
    if resolved.email != email {
        bail!("自动取码地址响应邮箱与当前账号不一致");
    }
    let expires = otp::parse_meta_timestamp(&resolved.expires_at)
        .filter(|_| resolved.expires_at.ends_with('Z'))
        .context("自动取码地址响应缺少有效 UTC 到期时间")?;
    if expires <= crate::now() {
        bail!("自动获取的取码链接已过期");
    }
    let code_url = parse_url(&resolved.code_url)?;
    if code_url.origin() != endpoint.origin() {
        bail!("自动取码地址与签发接口来源不一致");
    }
    Ok(code_url.into())
}
