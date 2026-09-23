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
    pub proxy_type: String,
    #[serde(default)]
    pub proxy_host: String,
    #[serde(default)]
    pub proxy_port: Option<i64>,
    #[serde(default)]
    pub remark: String,
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
