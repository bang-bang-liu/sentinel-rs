use anyhow::{bail, Result};
use std::{collections::HashMap, path::Path};

pub fn resolve(path: &Path, email: &str) -> Result<String> {
    // Reread only when this account actually receives an email challenge.
    let bytes = std::fs::read(path).map_err(|_| anyhow::anyhow!("无法读取重登邮箱映射文件"))?;
    let map: HashMap<String, String> = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("重登邮箱映射必须是邮箱到取码 URL 的 JSON 对象"))?;
    let raw = map
        .get(email)
        .ok_or_else(|| anyhow::anyhow!("此账号未配置重登邮箱映射"))?;
    let url = url::Url::parse(raw).map_err(|_| anyhow::anyhow!("此账号的取码 URL 格式无效"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        bail!("取码 URL 必须使用 HTTP 或 HTTPS");
    }
    Ok(raw.clone())
}
