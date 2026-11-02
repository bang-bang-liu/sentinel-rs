use serde::{Deserialize, Serialize};

pub const COUPONS: [&str; 4] = [
    "plus-1-month-free",
    "plus-1-month-50-pct-off",
    "plus-2-months-50-pct-off",
    "plus-3-months-50-pct-off",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Eligibility {
    Eligible,
    NotEligible,
    Unknown,
}

impl Eligibility {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Eligible => "eligible",
            Self::NotEligible => "not_eligible",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Outcome {
    pub eligibility: Eligibility,
    pub reason: String,
    pub http_status: Option<u16>,
    pub upstream_state: Option<String>,
    pub redemption_status: Option<String>,
    pub attempts: u32,
}

impl Outcome {
    pub fn unknown(reason: &str, status: Option<u16>) -> Self {
        Self {
            eligibility: Eligibility::Unknown,
            reason: reason.into(),
            http_status: status,
            upstream_state: None,
            redemption_status: None,
            attempts: 0,
        }
    }
}

pub fn classify(coupon: &str, status: u16, body: &str) -> Outcome {
    let reason = match status {
        200 => "",
        401 => "auth_required",
        403 => "access_denied",
        429 => "rate_limited",
        500..=599 => "upstream_error",
        _ => "unexpected_response",
    };
    if !reason.is_empty() {
        return Outcome::unknown(reason, Some(status));
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return Outcome::unknown("invalid_response", Some(status));
    };
    if value.get("coupon").and_then(|v| v.as_str()) != Some(coupon) {
        return Outcome::unknown("invalid_response", Some(status));
    }
    let state = value.get("state").and_then(|v| v.as_str()).unwrap_or("");
    let (eligibility, reason) = match state {
        "eligible" => (Eligibility::Eligible, "confirmed"),
        "not_eligible" => (Eligibility::NotEligible, "confirmed"),
        "offline" => (Eligibility::Unknown, "upstream_offline"),
        _ => (Eligibility::Unknown, "unexpected_response"),
    };
    Outcome {
        eligibility,
        reason: reason.into(),
        http_status: Some(status),
        upstream_state: match state {
            "eligible" | "not_eligible" | "offline" => Some(state.into()),
            _ => None,
        },
        redemption_status: None,
        attempts: 0,
    }
}

pub fn aggregate(values: impl IntoIterator<Item = Eligibility>) -> Eligibility {
    let values: Vec<_> = values.into_iter().collect();
    if values.contains(&Eligibility::Eligible) {
        Eligibility::Eligible
    } else if !values.is_empty() && values.iter().all(|v| *v == Eligibility::NotEligible) {
        Eligibility::NotEligible
    } else {
        Eligibility::Unknown
    }
}

pub fn valid_token(token: &str) -> bool {
    !token.is_empty() && token.len() <= 32768 && token.bytes().all(|b| (33..=126).contains(&b))
}
