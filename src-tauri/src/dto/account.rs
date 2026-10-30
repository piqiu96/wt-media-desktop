//! Account-check and scoped Cookie operation args/results. Cloud authorizes the operation;
//! the operation id groups the start and result writes.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize)]
pub struct AccountCheckArgs {
    pub operation_id: String,
    pub bit_profile_id: String,
    pub platform: String,
    #[serde(default)]
    pub expected_platform_account_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccountCheckResult {
    pub platform_account_id: String,
    pub name: String,
    pub avatar_url: String,
    pub login_status: String,
    pub message: String,
    #[serde(default)]
    pub check_items: Vec<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccountCookieStage {
    Prepare,
    Apply,
    Inspect,
}

#[derive(Clone, Debug, Deserialize)]
pub struct AccountCookieArgs {
    #[serde(default)]
    pub manual_login: bool,
    pub operation_id: String,
    pub bit_profile_id: String,
    pub platform: String,
    pub stage: AccountCookieStage,
    #[serde(default)]
    pub expected_platform_account_id: String,
    #[serde(default)]
    pub input_cookie: String,
    #[serde(default)]
    pub fingerprint: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccountCookieResult {
    #[serde(default)]
    pub verified: bool,
    pub status: String,
    #[serde(default)]
    pub applied: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_cookie: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cookies: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    pub fingerprint: String,
    #[serde(default)]
    pub platform_account_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub avatar_url: String,
    #[serde(default)]
    pub login_status: String,
    #[serde(default)]
    pub input_matches: bool,
    #[serde(default)]
    pub message: String,
}
