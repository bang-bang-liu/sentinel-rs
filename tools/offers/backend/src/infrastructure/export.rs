use crate::domain::model::Check;
use anyhow::{bail, Result};

pub fn csv_cell(value: &str) -> String {
    let stripped = value.trim_start_matches(|c: char| c.is_whitespace());
    let dangerous =
        stripped.starts_with(['=', '+', '-', '@']) || value.starts_with(['\t', '\r', '\n']);
    let safe = if dangerous {
        format!("'{value}")
    } else {
        value.to_owned()
    };
    format!("\"{}\"", safe.replace('"', "\"\""))
}
pub fn render(checks: &[Check], format: &str) -> Result<String> {
    fn iso(time: Option<i64>) -> Option<String> {
        time.and_then(|v| chrono::DateTime::from_timestamp(v, 0))
            .map(|v| v.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
    }
    if format == "json" {
        let rows = checks
            .iter()
            .map(|c| {
                let mut row = serde_json::to_value(c).unwrap();
                row["checked_at"] = serde_json::json!(iso(c.checked_at));
                row
            })
            .collect::<Vec<_>>();
        return Ok(serde_json::to_string_pretty(&rows)?);
    }
    if format != "csv" {
        bail!("invalid_export_format")
    }
    let mut out=String::from("\u{feff}account_id,email,coupon,execution_status,eligibility,reason,http_status,attempts,checked_at,task_id\r\n");
    for c in checks {
        let fields = [
            c.account_id.to_string(),
            c.email.clone(),
            c.coupon.clone(),
            c.execution_status.clone(),
            c.eligibility.map(|e| e.as_str().into()).unwrap_or_default(),
            c.reason.clone().unwrap_or_default(),
            c.http_status.map(|v| v.to_string()).unwrap_or_default(),
            c.attempts.to_string(),
            iso(c.checked_at).unwrap_or_default(),
            c.task_id.clone(),
        ];
        out.push_str(
            &fields
                .iter()
                .map(|v| csv_cell(v))
                .collect::<Vec<_>>()
                .join(","),
        );
        out.push_str("\r\n");
    }
    Ok(out)
}
