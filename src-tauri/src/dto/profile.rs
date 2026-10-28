//! BitBrowser profile operations: open/close/create/restore and the read-back
//! verification the restore flow reports to Vue.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize)]
pub struct ProfileOperationArgs {
    #[serde(alias = "bitProfileId")]
    pub bit_profile_id: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProfileOperationResult {
    pub bit_profile_id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CreateProfileArgs {
    pub name: String,
    pub group_id: String,
    #[serde(default)]
    pub group_name: String,
    #[serde(default)]
    pub seq: Option<i64>,
    #[serde(default)]
    pub remark: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct CreateProfileResult {
    pub bit_profile_id: String,
    pub snapshot: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize)]
pub struct UpdateProfileArgs {
    #[serde(alias = "bitProfileId")]
    pub bit_profile_id: String,
    pub name: String,
    pub group_id: String,
    #[serde(default)]
    pub group_name: String,
    #[serde(default)]
    pub remark: String,
}

/// 一个被修改的比特字段及写后扫描是否证实。比特可能只应用了部分内容，
/// 未能证实的字段标为 `unconfirmed`，而不是让整次编辑直接失败。
#[derive(Clone, Debug, Serialize)]
pub struct ProfileFieldVerification {
    pub field: String,
    pub status: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct UpdateProfileResult {
    pub bit_profile_id: String,
    pub fields: Vec<ProfileFieldVerification>,
    pub snapshot: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RestoreProfileInput {
    pub bit_profile_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub seq: Option<i64>,
    #[serde(default)]
    pub group_id: String,
    #[serde(default)]
    pub group_name: String,
    #[serde(default)]
    pub remark: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ProfileProxyOperationArgs {
    pub action: String,
    pub bit_profile_id: String,
    #[serde(default)]
    pub proxy_protocol: String,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub port: Option<i64>,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProfileProxyOperationResult {
    pub action: String,
    pub bit_profile_id: String,
    pub readback: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ProxyExtractArgs {
    pub extract_url: String,
    #[serde(default = "default_proxy_protocol")]
    pub proxy_protocol: String,
}

fn default_proxy_protocol() -> String {
    "http".into()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProxyExtractResult {
    pub proxy_protocol: String,
    pub host: String,
    pub port: i64,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct RestoreProfileResult {
    pub restored_count: usize,
    pub profiles: Vec<RestoreProfileVerification>,
    pub snapshot: serde_json::Value,
}

#[derive(Clone, Debug, Serialize)]
pub struct RestoreProfileVerification {
    pub bit_profile_id: String,
    pub status: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_operation_args_accepts_tauri_camel_case_payload() {
        let args: ProfileOperationArgs = serde_json::from_value(serde_json::json!({
            "bitProfileId": "profile-1"
        }))
        .unwrap();

        assert_eq!(args.bit_profile_id, "profile-1");
    }

    #[test]
    fn proxy_extract_args_defaults_the_protocol() {
        let args: ProxyExtractArgs = serde_json::from_value(serde_json::json!({
            "extract_url": "https://provider.example/extract"
        }))
        .unwrap();

        assert_eq!(args.proxy_protocol, "http");
    }

    #[test]
    fn profile_operation_result_can_omit_snapshot_for_open_close() {
        let result = ProfileOperationResult {
            bit_profile_id: "profile-1".into(),
            status: "opened".into(),
            snapshot: None,
        };

        let value = serde_json::to_value(result).unwrap();

        assert_eq!(value["bit_profile_id"], "profile-1");
        assert_eq!(value["status"], "opened");
        assert!(value.get("snapshot").is_none());
    }
}
