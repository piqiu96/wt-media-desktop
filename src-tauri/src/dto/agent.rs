//! Local Agent status surface: what `/api/v1/status` returns and what Vue
//! receives.
//!
//! Unknown fields in the Agent's answer are ignored: the status is a snapshot
//! for the page, and the page is not the place where an unreviewed field turns
//! into a displayed fact.
//!
//! `main_user_id` is read only to be masked. It is the BitBrowser account the
//! execution gate compares, so the page needs to know which account the machine
//! is on — but it can be told that without receiving the whole id: Cloud
//! publishes the bound account the same way, and the page compares two masks.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LocalAgentStatusResponse {
    pub data: LocalAgentStatusData,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LocalAgentStatusData {
    pub agent_id: String,
    pub status: String,
    #[serde(default)]
    pub bitbrowser_status: Option<String>,
    /// 本机 BitBrowser 正在登录的主账号。只用于下面那条掩码。
    #[serde(default)]
    pub main_user_id: Option<String>,
    #[serde(default)]
    pub operating_system: Option<String>,
    #[serde(default)]
    pub cpu_architecture: Option<String>,
    #[serde(default)]
    pub agent_version: Option<String>,
    #[serde(default)]
    pub python_version: Option<String>,
    #[serde(default)]
    pub ffmpeg: Option<RuntimeStatusFact>,
    #[serde(default)]
    pub workdir_status: Option<String>,
    #[serde(default)]
    pub disk: Option<RuntimeDiskFact>,
    #[serde(default)]
    pub bit_profile_ids: Vec<String>,
    #[serde(default)]
    pub current_task_id: Option<String>,
    #[serde(default)]
    pub current_task_progress: Option<u32>,
    #[serde(default)]
    pub current_task_status: Option<String>,
    #[serde(default)]
    pub pending_result_count: u32,
}

#[derive(Clone, Debug, Serialize)]
pub struct LocalAgentStatus {
    pub agent_id: String,
    pub status: String,
    pub bitbrowser_status: Option<String>,
    /// 本机主账号的掩码，规则与 Cloud 的 `maskMainUserID` 一致。
    pub main_user_id_masked: Option<String>,
    pub operating_system: Option<String>,
    pub cpu_architecture: Option<String>,
    pub agent_version: Option<String>,
    pub python_version: Option<String>,
    pub ffmpeg: Option<RuntimeStatusFact>,
    pub workdir_status: Option<String>,
    pub disk: Option<RuntimeDiskFact>,
    pub bit_profile_ids: Vec<String>,
    pub current_task_id: Option<String>,
    pub current_task_progress: Option<u32>,
    pub current_task_status: Option<String>,
    pub pending_result_count: u32,
}

/// 掩码规则与 Cloud 的 `maskMainUserID` 相同：前 4 + `****` + 后 4；8 位以内整体
/// 掩掉。两边各自掩自己那一半，页面只比对两个掩码。
///
/// 非 ASCII 时整体掩掉：那不是一个 BitBrowser 账号，按字节切分还可能切开一个字符。
pub fn mask_main_user_id(raw: Option<String>) -> Option<String> {
    let id = raw?.trim().to_string();
    if id.is_empty() {
        return None;
    }
    if !id.is_ascii() || id.len() <= 8 {
        return Some("****".into());
    }
    Some(format!("{}****{}", &id[..4], &id[id.len() - 4..]))
}

impl From<LocalAgentStatusData> for LocalAgentStatus {
    fn from(d: LocalAgentStatusData) -> Self {
        Self {
            agent_id: d.agent_id,
            status: d.status,
            bitbrowser_status: d.bitbrowser_status,
            main_user_id_masked: mask_main_user_id(d.main_user_id),
            operating_system: d.operating_system,
            cpu_architecture: d.cpu_architecture,
            agent_version: d.agent_version,
            python_version: d.python_version,
            ffmpeg: d.ffmpeg,
            workdir_status: d.workdir_status,
            disk: d.disk,
            bit_profile_ids: d.bit_profile_ids,
            current_task_id: d.current_task_id,
            current_task_progress: d.current_task_progress,
            current_task_status: d.current_task_status,
            pending_result_count: d.pending_result_count,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuntimeStatusFact {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuntimeDiskFact {
    pub status: String,
    pub free_megabytes: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 输入表与 Cloud `handler_test.go` 的 `TestMaskMainUserIDKeepsOnlyTheEdges`
    /// 逐条相同：两侧掩码一旦漂开，页面就会把同一个账号报成「不一致」。
    #[test]
    fn the_mask_matches_the_rule_cloud_publishes() {
        let cases = [
            (None, None),
            (Some(""), None),
            (Some("   "), None),
            (Some("short"), Some("****")),
            (Some("12345678"), Some("****")),
            (Some("123456789"), Some("1234****6789")),
            (Some("main-user-abcdef1234"), Some("main****1234")),
        ];
        for (input, want) in cases {
            assert_eq!(
                mask_main_user_id(input.map(str::to_string)),
                want.map(str::to_string),
                "mask_main_user_id({input:?})"
            );
        }
    }

    /// 页面拿到的状态里没有完整的本机主账号。
    #[test]
    fn the_page_sees_the_mask_and_never_the_whole_account() {
        let raw = r#"{"data":{"agent_id":"local-agent-dev","status":"idle","main_user_id":"2c9b000000000000000000000000619f"}}"#;
        let answer: LocalAgentStatusResponse =
            serde_json::from_str(raw).expect("the Agent's shape");

        let status = LocalAgentStatus::from(answer.data);

        assert_eq!(status.main_user_id_masked.as_deref(), Some("2c9b****619f"));
        let page = serde_json::to_string(&status).expect("serializable");
        assert!(!page.contains("2c9b000000000000000000000000619f"), "{page}");
        assert!(
            page.contains(r#""main_user_id_masked":"2c9b****619f""#),
            "{page}"
        );
    }
}
