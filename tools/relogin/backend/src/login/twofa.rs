use super::protocol::{self, Step, Transport};
use anyhow::{bail, Result};
use sentinel_rs::flow::{
    auth::AuthRequestBuilder, http::RequestSpec, twofa, SentinelToken, SentinelTokenSource,
};
use std::sync::Arc;

pub async fn verify(
    transport: &dyn Transport,
    builder: &AuthRequestBuilder,
    source: Arc<dyn SentinelTokenSource>,
    device_id: &str,
    secret: &str,
    factor_id: &str,
) -> Result<Step> {
    if secret.trim().is_empty() {
        bail!("登录要求 TOTP，但本地账号没有 TOTP 密钥");
    }
    if factor_id.is_empty() {
        bail!("TOTP 挑战未返回 factor ID");
    }
    let mut headers = builder.common_json_headers("https://auth.openai.com/log-in/password");
    headers.push(("content-type".into(), "application/json".into()));
    headers.push(("accept".into(), "*/*".into()));
    let issue = RequestSpec::post_json(
        "relogin.mfa_issue_challenge",
        "https://auth.openai.com/api/accounts/mfa/issue_challenge",
        Some(headers),
        Some(serde_json::json!({"id":factor_id,"type":"totp","force_fresh_challenge":false})),
    );
    let response = protocol::request(transport, &issue).await?;
    protocol::success(&response, "准备 TOTP 挑战")?;
    let st = protocol::token(source, "password_verify", device_id).await?;
    let code =
        twofa::totp_now(secret).map_err(|_| anyhow::anyhow!("本地 TOTP 密钥无法生成验证码"))?;
    let mut spec = builder.mfa_verify(&code, factor_id, &st);
    spec.set_header(
        "referer",
        &format!("https://auth.openai.com/mfa-challenge/{factor_id}"),
    );
    protocol::step_request(transport, &spec).await
}

pub async fn enroll(
    transport: &dyn Transport,
    builder: &AuthRequestBuilder,
    access_token: &str,
) -> Result<String> {
    // These existing builders do not use SentinelToken. Preserve registration's enrollment order
    // and activation completion rule, without mfa_info or an activation recheck.
    let st = SentinelToken {
        token: String::new(),
        so_token: None,
    };
    let response =
        protocol::request(transport, &twofa::enroll_spec(builder, access_token, &st)).await?;
    let enrollment = twofa::parse_enroll(&response.body)
        .map_err(|_| anyhow::anyhow!("2FA enroll 未返回密钥及 session ID"))?;
    let code = twofa::totp_now(&enrollment.secret)
        .map_err(|_| anyhow::anyhow!("无法计算新 2FA 动态码"))?;
    let spec = twofa::activate_spec(builder, access_token, &code, &enrollment.session_id, &st);
    protocol::request(transport, &spec).await?;
    Ok(enrollment.secret)
}
