//! Local Agent status surface: what `/api/v1/status` returns, what Vue
//! receives, and what Desktop reports back to Cloud as runtime facts.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LocalAgentStatusResponse {
    pub data: LocalAgentStatusData,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LocalAgentStatusData {
    #[serde(default)]
    pub node_id: Option<String>,
    pub agent_id: String,
    pub status: String,
    #[serde(default)]
    pub bitbrowser_status: Option<String>,
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
    pub node_id: Option<String>,
    pub agent_id: String,
    pub status: String,
    pub bitbrowser_status: Option<String>,
    pub main_user_id: Option<String>,
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

impl From<LocalAgentStatusData> for LocalAgentStatus {
    fn from(d: LocalAgentStatusData) -> Self {
        Self {
            node_id: d.node_id,
            agent_id: d.agent_id,
            status: d.status,
            bitbrowser_status: d.bitbrowser_status,
            main_user_id: d.main_user_id,
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

#[derive(Clone, Debug, Serialize)]
pub struct RuntimeReportRequest {
    pub operating_system: String,
    pub cpu_architecture: String,
    pub agent_version: String,
    pub python_version: String,
    pub ffmpeg: RuntimeStatusFact,
    pub workdir_status: String,
    pub disk: RuntimeDiskFact,
    pub bitbrowser_status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub main_user_id: Option<String>,
    #[serde(default)]
    pub bit_profile_ids: Vec<String>,
}
