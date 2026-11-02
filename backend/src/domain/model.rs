use super::campaign::Eligibility;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Account {
    pub id: i64,
    pub email: String,
    pub status: String,
    pub has_token: bool,
}

// Credentials intentionally have neither Debug nor Serialize implementations.
pub struct Credential {
    pub access_token: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Check {
    pub task_id: String,
    pub account_id: i64,
    pub email: String,
    pub coupon: String,
    pub execution_status: String,
    pub eligibility: Option<Eligibility>,
    pub reason: Option<String>,
    pub http_status: Option<u16>,
    pub upstream_state: Option<String>,
    pub attempts: u32,
    pub checked_at: Option<i64>,
    pub redemption_status: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Task {
    pub task_id: String,
    pub status: String,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub stop_reason: Option<String>,
    pub retry_after_at: Option<i64>,
    pub total_accounts: usize,
    pub completed_accounts: usize,
    pub total_checks: usize,
    pub completed_checks: usize,
    pub unknown_checks: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct AccountView {
    #[serde(flatten)]
    pub account: Account,
    pub free_trial: Eligibility,
    pub half_price: Eligibility,
    pub latest_task_id: Option<String>,
    pub checked_at: Option<i64>,
    pub checks: Vec<Check>,
    pub last_confirmed: Vec<Check>,
}

#[derive(Default, Serialize)]
pub struct Summary {
    pub total: usize,
    pub missing_token: usize,
    pub free_eligible: usize,
    pub half_eligible: usize,
    pub pending: usize,
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
