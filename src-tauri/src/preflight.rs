//! The Cloud preflight handshake the sensitive flows share, and the guards
//! around it.
//!
//! `local_agent_account_check` and `local_agent_cookie_read` each carried their
//! own copy of this block, differing in how they name the operation and in one
//! sentence about a missing Cloud address. Both copies are gone. What they used
//! to say is pinned by `tests::message_parity`, because a reworded error string
//! is a user-visible change, not a cleanup.
//!
//! Wording that legitimately differs per flow travels as **text**
//! (`no_cloud_address`, `no_binding`) rather than through a template: four flows
//! word those two sentences four different ways, and a template that covered all
//! four would be a grammar with four sentences in it. The constants below are
//! that text, collected in one place so the parity test can reach every one of
//! them.
//!
//! What the tests here do **not** cover: the async paths (`run`,
//! `sync_runtime_facts`, `finish_permit`) all speak HTTP, so they are exercised
//! against the real Local Agent and Cloud in the CHG's acceptance tooling, not
//! here. This module's tests cover the pure wording and guard logic.

use crate::dto::{
    CloudEnvelope, GuardPreflightOutcome, LocalAgentStatus, RuntimeDiskFact, RuntimeReportRequest,
    RuntimeStatusFact,
};
use crate::http::CloudClient;
use crate::state::{RuntimeBinding, RuntimeBindingState};
use reqwest::StatusCode;

/// Which sensitive operation a preflight is for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreflightSpec {
    noun: &'static str,
    no_cloud_address: &'static str,
}

impl PreflightSpec {
    /// The sentence to refuse an empty Cloud address with.
    ///
    /// Public because the commands reject that address *before* the preflight
    /// runs, while the rest of the spec is only used inside this module.
    pub fn no_cloud_address(self) -> &'static str {
        self.no_cloud_address
    }
}

pub const ACCOUNT_CHECK: PreflightSpec = PreflightSpec {
    noun: "账号检查",
    no_cloud_address: "Cloud地址为空，无法执行账号检查",
};

pub const COOKIE_READ: PreflightSpec = PreflightSpec {
    noun: "Cookie读取",
    no_cloud_address: "Cloud地址为空，无法读取Cookie",
};

/// The "no Cloud address" sentence for the two flows that are not sensitive
/// tasks and so have no `PreflightSpec`.
pub const NO_CLOUD_ADDRESS_BIND: &str = "Cloud地址为空，无法完成本机可信绑定";
pub const NO_CLOUD_ADDRESS_REFRESH: &str = "Cloud地址为空，无法刷新本机可信状态";

/// The "this machine is not bound yet" sentence. It differs by flow because
/// they ask for different next steps: `refresh_runtime` tells the user to bind,
/// while the sensitive flows tell them to re-detect and then bind.
pub const NO_BINDING_SENSITIVE: &str = "当前电脑尚未完成可信绑定，请先到环境状态页重新检测并绑定";
pub const NO_BINDING_REFRESH: &str =
    "当前电脑尚未完成可信绑定，请先到环境状态页绑定当前比特浏览器账号";

/// The "missing BitBrowser identity" sentences. Shared by every flow, so they
/// are constants rather than parameters.
pub const NO_MAIN_USER_ID: &str = "未读取到BitBrowser主账号，请确认BitBrowser已登录后重新检测";
pub const BITBROWSER_UNUSABLE: &str = "BitBrowser不可用或身份不可验证，请处理后重新检测";

/// The permit-release failure sentence.
///
/// It names the account-check flow even when a cookie read is being released,
/// because that is what it has always said and this refactor holds the wording
/// still. Registered as D-09: making it spec-aware is a one-word change, and it
/// is deliberately not bundled into a commit whose promise is "no user-visible
/// text changes".
pub const RELEASE_FAILED: &str = "释放账号检查本机授权失败";

/// A preflight that did not end in a granted permit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PreflightFailure {
    /// The request never reached Cloud.
    Transport(String),
    /// Cloud answered with a non-success status.
    Status(StatusCode, String),
    /// Cloud answered successfully with a body this client cannot read.
    Body(String),
    /// Cloud read the request but refused it; carries Cloud's own message, which
    /// may be empty.
    CloudRefused(String),
    /// Cloud granted something, but without an outcome object.
    MissingOutcome,
    /// Cloud is holding the window for another sensitive operation.
    NotGranted,
    /// Cloud granted the permit without the local execution credential.
    MissingPermitCredential,
}

impl PreflightFailure {
    /// The exact sentence this failure has always produced for `spec`.
    pub fn message(&self, spec: PreflightSpec) -> String {
        let noun = spec.noun;
        match self {
            PreflightFailure::Transport(reason) => format!("{}预检失败: {}", noun, reason),
            PreflightFailure::Status(status, text) => {
                format!("{}预检失败: {} {}", noun, status, text)
            }
            PreflightFailure::Body(reason) => format!("{}预检响应格式错误: {}", noun, reason),
            PreflightFailure::CloudRefused(reason) => {
                if reason.is_empty() {
                    format!("{}预检失败", noun)
                } else {
                    reason.clone()
                }
            }
            PreflightFailure::MissingOutcome => format!("{}预检缺少授权结果", noun),
            // The one sentence that does not mention the operation: it is about
            // the window, not about what was being attempted.
            PreflightFailure::NotGranted => "当前窗口正在执行其他敏感操作，请稍后重试".to_string(),
            PreflightFailure::MissingPermitCredential => {
                format!("{}授权缺少本机执行凭证", noun)
            }
        }
    }
}

/// Normalise a Cloud base URL the one way all four flows do, or refuse with the
/// caller's own sentence.
pub fn require_cloud_base_url(raw: &str, no_cloud_address: &str) -> Result<String, String> {
    let url = raw.trim().trim_end_matches('/').to_string();
    if url.is_empty() {
        return Err(no_cloud_address.to_string());
    }
    Ok(url)
}

/// Take the stored node binding, or refuse with the caller's own sentence.
pub fn require_binding(
    state: &RuntimeBindingState,
    no_binding: &str,
) -> Result<RuntimeBinding, String> {
    state
        .0
        .lock()
        .map_err(|_| "runtime binding lock poisoned".to_string())?
        .clone()
        .ok_or_else(|| no_binding.to_string())
}

/// A runtime report is only meaningful for a machine whose BitBrowser identity
/// can be read and verified, so both are required before one is sent.
///
/// This was inlined at two sites; the flows that reach it are four
/// (`bind_session`, `refresh_runtime`, and the two sensitive tasks). The two
/// sites were byte-identical, guarded check included.
pub fn require_verified_bitbrowser(status: &LocalAgentStatus) -> Result<(), String> {
    let main_user_id = status.main_user_id.clone().unwrap_or_default();
    if main_user_id.trim().is_empty() {
        return Err(NO_MAIN_USER_ID.to_string());
    }
    if status.bitbrowser_status.as_deref() != Some("normal") {
        return Err(BITBROWSER_UNUSABLE.to_string());
    }
    Ok(())
}

/// Report this machine's runtime facts to Cloud.
pub async fn sync_runtime_facts(
    cloud: &CloudClient,
    cloud_base_url: &str,
    node_id: &str,
    node_credential: &str,
    status: &LocalAgentStatus,
) -> Result<(), String> {
    require_verified_bitbrowser(status)?;
    let main_user_id = status.main_user_id.clone().unwrap_or_default();
    let runtime_report = RuntimeReportRequest {
        operating_system: status
            .operating_system
            .clone()
            .unwrap_or_else(|| "unsupported".into()),
        cpu_architecture: status
            .cpu_architecture
            .clone()
            .unwrap_or_else(|| "unsupported".into()),
        agent_version: status
            .agent_version
            .clone()
            .unwrap_or_else(|| "0.2.2".into()),
        python_version: status
            .python_version
            .clone()
            .unwrap_or_else(|| "unknown".into()),
        ffmpeg: status.ffmpeg.clone().unwrap_or(RuntimeStatusFact {
            status: "unreachable".into(),
            version: None,
        }),
        workdir_status: status
            .workdir_status
            .clone()
            .unwrap_or_else(|| "normal".into()),
        disk: status.disk.clone().unwrap_or(RuntimeDiskFact {
            status: "normal".into(),
            free_megabytes: 0,
        }),
        bitbrowser_status: status
            .bitbrowser_status
            .clone()
            .unwrap_or_else(|| "unknown".into()),
        main_user_id: Some(main_user_id),
        bit_profile_ids: status.bit_profile_ids.clone(),
    };
    let report_url = format!(
        "{}/api/v1/local-agent/nodes/{}/runtime-report",
        cloud_base_url, node_id
    );
    let report_resp = cloud
        .post(&report_url)
        .bearer_auth(node_credential)
        .json(&runtime_report)
        .send()
        .await
        .map_err(|e| format!("本机可信状态刷新失败: {}", e))?;
    if report_resp.status().is_success() {
        return Ok(());
    }
    let status_code = report_resp.status();
    let text = report_resp.text().await.unwrap_or_default();
    Err(format!("本机可信状态刷新失败: {} {}", status_code, text))
}

/// Ask Cloud for permission to run a sensitive task, and return the granted
/// permit.
///
/// The check order is load-bearing: Cloud's own refusal message is preferred
/// over a generic one, and a permit missing its credential is rejected before
/// anything local is attempted.
pub async fn run(
    cloud: &CloudClient,
    cloud_base_url: &str,
    task_id: &str,
    binding: &RuntimeBinding,
    spec: PreflightSpec,
) -> Result<GuardPreflightOutcome, String> {
    let fail = |failure: PreflightFailure| failure.message(spec);
    let url = format!(
        "{}/api/v1/local-agent/sensitive-tasks/{}/preflight",
        cloud_base_url, task_id
    );
    let resp = cloud
        .post(&url)
        .bearer_auth(&binding.node_credential)
        .json(&serde_json::json!({"node_id": binding.node_id}))
        .send()
        .await
        .map_err(|e| fail(PreflightFailure::Transport(e.to_string())))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(fail(PreflightFailure::Status(status, text)));
    }
    let body: CloudEnvelope<GuardPreflightOutcome> =
        serde_json::from_str(&text).map_err(|e| fail(PreflightFailure::Body(e.to_string())))?;
    if body.errcode != 0 {
        return Err(fail(PreflightFailure::CloudRefused(body.message)));
    }
    let permit = body
        .data
        .ok_or_else(|| fail(PreflightFailure::MissingOutcome))?;
    if permit.outcome != "granted" {
        return Err(fail(PreflightFailure::NotGranted));
    }
    if permit.permit_id.is_empty() || permit.permit_credential.is_empty() {
        return Err(fail(PreflightFailure::MissingPermitCredential));
    }
    Ok(permit)
}

/// Compose the release-failure sentence the way both flows composed it: the
/// constant, a colon, then whatever detail the failure carries.
///
/// Split out from `finish_permit` so the composition is reachable by a test —
/// `RELEASE_FAILED` alone does not pin the `": "` or the two-detail form, and
/// both were spelled inline in the original.
fn release_failure(detail: impl std::fmt::Display) -> String {
    format!("{}: {}", RELEASE_FAILED, detail)
}

/// Hand a permit back. Called even when the local operation failed, with
/// `result_uncertain`, so Cloud never keeps a permit whose outcome nobody knows.
pub async fn finish_permit(
    cloud: &CloudClient,
    cloud_base_url: &str,
    binding: &RuntimeBinding,
    permit_id: &str,
    permit_credential: &str,
    outcome: &str,
) -> Result<(), String> {
    let finish_url = format!(
        "{}/api/v1/local-agent/sensitive-permits/{}/finish",
        cloud_base_url, permit_id
    );
    let resp = cloud
        .post(&finish_url)
        .bearer_auth(&binding.node_credential)
        .header("X-Profile-Permit", permit_credential)
        .json(&serde_json::json!({"node_id": binding.node_id, "outcome": outcome}))
        .send()
        .await
        .map_err(release_failure)?;
    if resp.status().is_success() {
        return Ok(());
    }
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    Err(release_failure(format!("{} {}", status, text)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every sentence this module can produce, checked against the wording the
    /// pre-dedup code produced.
    ///
    /// The expected column was read off the two command bodies before they were
    /// merged (`git show <pre-refactor>:src-tauri/src/commands/account.rs`,
    /// lines 59-80 and 176-197) and off the two guard sites in `bind`, not off
    /// this module's output — an expectation taken from the implementation would
    /// make this test a tautology. `account_check` and `cookie_read` differ only
    /// in the noun, so each row appears twice.
    ///
    /// Both `Status` rows say `400 Bad Request`, not `400`: the flows formatted
    /// `reqwest::StatusCode` with `{}`, and its `Display` includes the reason
    /// phrase. The first draft of this table guessed the bare code and went red
    /// here — which is the failure this test exists to produce.
    #[test]
    fn message_parity() {
        use PreflightFailure::*;

        // All seven variants for both specs, plus the empty-refusal branch for
        // both: 16 rows, so no variant is left unpinned for either flow.
        let rows: [(PreflightSpec, PreflightFailure, &str); 16] = [
            // account_check
            (
                ACCOUNT_CHECK,
                Transport("boom".into()),
                "账号检查预检失败: boom",
            ),
            (
                ACCOUNT_CHECK,
                Status(StatusCode::BAD_REQUEST, "body".into()),
                "账号检查预检失败: 400 Bad Request body",
            ),
            (
                ACCOUNT_CHECK,
                Body("bad json".into()),
                "账号检查预检响应格式错误: bad json",
            ),
            (ACCOUNT_CHECK, CloudRefused("拒绝".into()), "拒绝"),
            (
                ACCOUNT_CHECK,
                CloudRefused(String::new()),
                "账号检查预检失败",
            ),
            (ACCOUNT_CHECK, MissingOutcome, "账号检查预检缺少授权结果"),
            (
                ACCOUNT_CHECK,
                NotGranted,
                "当前窗口正在执行其他敏感操作，请稍后重试",
            ),
            (
                ACCOUNT_CHECK,
                MissingPermitCredential,
                "账号检查授权缺少本机执行凭证",
            ),
            // cookie_read
            (
                COOKIE_READ,
                Transport("boom".into()),
                "Cookie读取预检失败: boom",
            ),
            (
                COOKIE_READ,
                Status(StatusCode::BAD_REQUEST, "body".into()),
                "Cookie读取预检失败: 400 Bad Request body",
            ),
            (
                COOKIE_READ,
                Body("bad json".into()),
                "Cookie读取预检响应格式错误: bad json",
            ),
            (COOKIE_READ, CloudRefused("拒绝".into()), "拒绝"),
            (
                COOKIE_READ,
                CloudRefused(String::new()),
                "Cookie读取预检失败",
            ),
            (COOKIE_READ, MissingOutcome, "Cookie读取预检缺少授权结果"),
            (
                COOKIE_READ,
                NotGranted,
                "当前窗口正在执行其他敏感操作，请稍后重试",
            ),
            (
                COOKIE_READ,
                MissingPermitCredential,
                "Cookie读取授权缺少本机执行凭证",
            ),
        ];

        for (spec, failure, expected) in rows {
            assert_eq!(failure.message(spec), expected, "{failure:?}");
        }
    }

    /// The `Status` variant renders the code through `Display`, exactly as the
    /// flows did when they formatted `reqwest::StatusCode` with `{}` — reason
    /// phrase included, and with the separator space still there when the body
    /// is empty.
    ///
    /// That trailing space is the point of keeping this test: a "tidy up" that
    /// trims it, or that switches to `as_u16()`, changes user-visible text, and
    /// the row above would not notice because it only ever has a non-empty body.
    #[test]
    fn status_is_rendered_by_display_including_the_reason_phrase() {
        assert_eq!(
            PreflightFailure::Status(StatusCode::INTERNAL_SERVER_ERROR, String::new())
                .message(ACCOUNT_CHECK),
            "账号检查预检失败: 500 Internal Server Error "
        );
    }

    #[test]
    fn cloud_base_url_is_normalized_then_checked() {
        assert_eq!(
            require_cloud_base_url("  http://127.0.0.1:18080/  ", NO_CLOUD_ADDRESS_BIND).unwrap(),
            "http://127.0.0.1:18080"
        );
        assert_eq!(
            require_cloud_base_url("http://127.0.0.1:18080//", NO_CLOUD_ADDRESS_REFRESH).unwrap(),
            "http://127.0.0.1:18080"
        );

        // Every spelling of "empty" the four flows can receive, including the
        // ones that only become empty after the trailing-slash trim.
        for (raw, sentence) in [
            ("", NO_CLOUD_ADDRESS_BIND),
            ("   ", NO_CLOUD_ADDRESS_REFRESH),
            ("/", NO_CLOUD_ADDRESS_BIND),
            ("//", NO_CLOUD_ADDRESS_REFRESH),
        ] {
            assert_eq!(
                require_cloud_base_url(raw, sentence),
                Err(sentence.to_string()),
                "raw {raw:?} must refuse with the caller's sentence, not a shared one"
            );
        }
    }

    #[test]
    fn binding_guard_uses_the_callers_sentence() {
        let empty = RuntimeBindingState::default();
        for sentence in [NO_BINDING_SENSITIVE, NO_BINDING_REFRESH] {
            let error =
                require_binding(&empty, sentence).expect_err("an unbound runtime must refuse");
            assert_eq!(
                error, sentence,
                "the guard must not substitute its own wording"
            );
        }
    }

    #[test]
    fn identity_guard_accepts_a_readable_normal_bitbrowser() {
        // Spelled out rather than `..Default::default()`: the DTO carries no
        // `Default`, and adding one to production code to shorten a test is the
        // wrong trade.
        let status = |main_user_id: Option<&str>, bitbrowser: Option<&str>| LocalAgentStatus {
            node_id: None,
            agent_id: "agent-1".into(),
            status: "online".into(),
            bitbrowser_status: bitbrowser.map(str::to_string),
            main_user_id: main_user_id.map(str::to_string),
            operating_system: None,
            cpu_architecture: None,
            agent_version: None,
            python_version: None,
            ffmpeg: None,
            workdir_status: None,
            disk: None,
            bit_profile_ids: Vec::new(),
            current_task_id: None,
            current_task_progress: None,
            current_task_status: None,
            pending_result_count: 0,
        };

        // Positive control first: a guard that rejects everything would satisfy
        // every case below.
        assert!(require_verified_bitbrowser(&status(Some("user-1"), Some("normal"))).is_ok());

        for (case, value) in [
            ("missing主账号", status(None, Some("normal"))),
            ("blank主账号", status(Some("  "), Some("normal"))),
            (
                "BitBrowser未normal",
                status(Some("user-1"), Some("abnormal")),
            ),
            ("BitBrowser状态缺失", status(Some("user-1"), None)),
        ] {
            assert_eq!(
                require_verified_bitbrowser(&value),
                Err(if case.starts_with("BitBrowser") {
                    BITBROWSER_UNUSABLE.to_string()
                } else {
                    NO_MAIN_USER_ID.to_string()
                }),
                "{case}"
            );
        }
    }

    /// Every divergent sentence is a distinct constant, so the parity test can
    /// reach all of them and a collapsed-together pair is visible here.
    #[test]
    fn divergent_wording_is_not_shared() {
        let sentences = [
            ACCOUNT_CHECK.no_cloud_address,
            COOKIE_READ.no_cloud_address,
            NO_CLOUD_ADDRESS_BIND,
            NO_CLOUD_ADDRESS_REFRESH,
            NO_BINDING_SENSITIVE,
            NO_BINDING_REFRESH,
            NO_MAIN_USER_ID,
            BITBROWSER_UNUSABLE,
            RELEASE_FAILED,
        ];
        let mut unique = sentences.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), sentences.len(), "{sentences:?}");
    }

    /// Pins the D-09 oddity so that fixing it is a deliberate edit.
    #[test]
    fn release_text_still_names_the_account_check_flow() {
        assert_eq!(RELEASE_FAILED, "释放账号检查本机授权失败");
        assert!(!RELEASE_FAILED.contains(COOKIE_READ.noun));
    }

    /// Both shapes the original spelled inline: the transport failure
    /// (`"释放账号检查本机授权失败: {}"`) and the non-success one
    /// (`"释放账号检查本机授权失败: {} {}"`). The constant alone would not
    /// catch a dropped colon or a tightened separator, and the byte-parity
    /// sweep cannot see them either — those two literals do not survive
    /// verbatim once the constant is factored out.
    #[test]
    fn release_failure_composes_the_flows_own_sentence() {
        assert_eq!(release_failure("boom"), "释放账号检查本机授权失败: boom");
        assert_eq!(
            release_failure(format!("{} {}", StatusCode::INTERNAL_SERVER_ERROR, "body")),
            "释放账号检查本机授权失败: 500 Internal Server Error body"
        );
        // Empty detail keeps the separator, matching the original's `{} {}`
        // with a body-less response.
        assert_eq!(
            release_failure(format!("{} {}", StatusCode::NOT_FOUND, "")),
            "释放账号检查本机授权失败: 404 Not Found "
        );
    }
}
