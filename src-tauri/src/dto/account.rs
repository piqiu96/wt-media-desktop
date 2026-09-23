//! Account-check and cookie-read args/results. Both are sensitive-task flows
//! gated by a Cloud permit.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize)]
pub struct AccountCheckArgs {
    pub cloud_base_url: String,
    pub task_id: String,
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
pub struct CookieReadArgs {
    pub cloud_base_url: String,
    pub task_id: String,
    pub bit_profile_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CookieReadData {
    pub cookies: Vec<serde_json::Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CookieReadResult {
    pub cookies: Vec<serde_json::Value>,
    pub cookie_count: usize,
}
