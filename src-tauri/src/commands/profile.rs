//! BitBrowser profile commands: scan, groups, open/close, create and restore.
//!
//! Desktop owns this local bridge. The Vue layer must not call the Local Agent
//! port or hold Local Agent credentials directly.

use crate::dto::{
    CreateProfileArgs, CreateProfileResult, GuardRefusal, ProfileFieldVerification,
    ProfileOperationArgs, ProfileOperationResult, ProfileProxyOperationArgs,
    ProfileProxyOperationResult, ProxyExtractArgs, ProxyExtractResult, RestoreProfileInput,
    RestoreProfileResult, RestoreProfileVerification, UpdateProfileArgs, UpdateProfileResult,
};
use crate::http::LocalAgentClient;
use tauri::State;

/// Read the current BitBrowser Profile snapshot through the Local Agent.
///
/// Desktop owns this local bridge. The Vue layer must not call the Local Agent
/// port or hold Local Agent credentials directly.
#[tauri::command]
pub async fn local_agent_profile_scan(
    client: State<'_, LocalAgentClient>,
) -> Result<serde_json::Value, GuardRefusal> {
    let resp = client
        .post("/api/v1/bit-browser/profile-scans")
        .send()
        .await
        .map_err(GuardRefusal::unreachable)?;
    let status = resp.status();
    let text = resp.text().await.map_err(GuardRefusal::unreachable)?;
    if !status.is_success() {
        return Err(GuardRefusal::from_agent(status, &text));
    }
    serde_json::from_str(&text).map_err(|_| GuardRefusal::unreadable(status))
}
#[tauri::command]
pub async fn local_agent_profile_groups(
    client: State<'_, LocalAgentClient>,
) -> Result<serde_json::Value, GuardRefusal> {
    let resp = client
        .post("/api/v1/bit-browser/profile-groups")
        .send()
        .await
        .map_err(GuardRefusal::unreachable)?;
    let status = resp.status();
    let text = resp.text().await.map_err(GuardRefusal::unreachable)?;
    if !status.is_success() {
        return Err(GuardRefusal::from_agent(status, &text));
    }
    serde_json::from_str(&text).map_err(|_| GuardRefusal::unreadable(status))
}
#[tauri::command]
pub async fn local_agent_profile_open(
    client: State<'_, LocalAgentClient>,
    args: ProfileOperationArgs,
) -> Result<ProfileOperationResult, GuardRefusal> {
    let bit_profile_id = args.bit_profile_id.trim().to_string();
    if bit_profile_id.is_empty() {
        return Err(profile_refusal(
            "profile_id_required",
            "打开窗口失败：缺少BitBrowser窗口ID",
        ));
    }
    post_local_agent_profile_operation(&client, "profile-open", &bit_profile_id).await?;
    Ok(ProfileOperationResult {
        bit_profile_id,
        status: "opened".into(),
        snapshot: None,
    })
}
#[tauri::command]
pub async fn local_agent_profile_close(
    client: State<'_, LocalAgentClient>,
    args: ProfileOperationArgs,
) -> Result<ProfileOperationResult, GuardRefusal> {
    let bit_profile_id = args.bit_profile_id.trim().to_string();
    if bit_profile_id.is_empty() {
        return Err(profile_refusal(
            "profile_id_required",
            "关闭窗口失败：缺少BitBrowser窗口ID",
        ));
    }
    post_local_agent_profile_operation(&client, "profile-close", &bit_profile_id).await?;
    Ok(ProfileOperationResult {
        bit_profile_id,
        status: "closed".into(),
        snapshot: None,
    })
}
#[tauri::command]
pub async fn local_agent_profile_create(
    client: State<'_, LocalAgentClient>,
    args: CreateProfileArgs,
) -> Result<CreateProfileResult, GuardRefusal> {
    let payload = create_profile_payload(&args)
        .map_err(|message| profile_refusal("profile_input_invalid", message))?;
    let resp = client
        .post("/api/v1/bit-browser/profile-create")
        .json(&payload)
        .send()
        .await
        .map_err(GuardRefusal::unreachable)?;
    let status = resp.status();
    let text = resp.text().await.map_err(GuardRefusal::unreachable)?;
    if !status.is_success() {
        return Err(GuardRefusal::from_agent(status, &text));
    }
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|_| GuardRefusal::unreadable(status))?;
    let bit_profile_id = value
        .get("data")
        .and_then(|data| data.get("id"))
        .and_then(|id| id.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    let creation_status = value
        .get("data")
        .and_then(|d| d.get("status"))
        .and_then(|s| s.as_str())
        .unwrap_or("created")
        .to_string();
    if !args.creation_marker.is_empty()
        && (creation_status == "uncertain" || creation_status == "unavailable")
    {
        return Ok(CreateProfileResult {
            status: creation_status,
            bit_profile_id,
            snapshot: serde_json::Value::Null,
        });
    }
    if bit_profile_id.is_empty() {
        return Err(profile_refusal(
            "profile_readback_unverified",
            "创建BitBrowser窗口后未返回窗口ID",
        ));
    }
    let snapshot = local_agent_profile_scan(client.clone()).await?;
    verify_snapshot_has_profile(&snapshot, &bit_profile_id)
        .map_err(|message| profile_refusal("profile_readback_unverified", message))?;
    Ok(CreateProfileResult {
        status: creation_status,
        bit_profile_id,
        snapshot,
    })
}
/// 修改窗口在比特中维护的字段，随后扫描一次，逐字段报告已核实或仍待确认。
#[tauri::command]
pub async fn local_agent_profile_update(
    client: State<'_, LocalAgentClient>,
    args: UpdateProfileArgs,
) -> Result<UpdateProfileResult, GuardRefusal> {
    let bit_profile_id = args.bit_profile_id.trim().to_string();
    if bit_profile_id.is_empty() {
        return Err(profile_refusal(
            "profile_id_required",
            "编辑窗口失败：缺少BitBrowser窗口ID",
        ));
    }
    if args.name.trim().is_empty() {
        return Err(profile_refusal(
            "profile_input_invalid",
            "编辑窗口失败：窗口名称不能为空",
        ));
    }
    let resp = client
        .post("/api/v1/bit-browser/profile-update")
        .json(&update_profile_payload(&args))
        .send()
        .await
        .map_err(GuardRefusal::unreachable)?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.map_err(GuardRefusal::unreachable)?;
        return Err(GuardRefusal::from_agent(status, &text));
    }
    let snapshot = local_agent_profile_scan(client.clone()).await?;
    Ok(UpdateProfileResult {
        bit_profile_id: bit_profile_id.clone(),
        fields: verify_updated_profile(&args, &snapshot),
        snapshot,
    })
}

fn update_profile_payload(args: &UpdateProfileArgs) -> serde_json::Value {
    let mut payload = serde_json::json!({
        "id": args.bit_profile_id.trim(),
        "name": args.name.trim(),
        "remark": args.remark.trim(),
    });
    // 既有窗口可能尚未分组。未选择分组时保留比特原值，备注等字段仍可编辑。
    if !args.group_id.trim().is_empty() {
        payload["groupId"] = serde_json::Value::String(args.group_id.trim().to_string());
        payload["groupName"] = serde_json::Value::String(args.group_name.trim().to_string());
    }
    payload
}

fn verify_updated_profile(
    args: &UpdateProfileArgs,
    snapshot: &serde_json::Value,
) -> Vec<ProfileFieldVerification> {
    let expected = [
        ("name", args.name.trim().to_string()),
        ("group_id", args.group_id.trim().to_string()),
        ("group_name", args.group_name.trim().to_string()),
        ("remark", args.remark.trim().to_string()),
    ];
    let scanned = snapshot
        .get("profiles")
        .and_then(|value| value.as_array())
        .and_then(|profiles| {
            profiles.iter().find(|profile| {
                string_field(profile, "bit_profile_id") == args.bit_profile_id.trim()
            })
        });
    expected
        .iter()
        .map(|(field, value)| ProfileFieldVerification {
            field: (*field).to_string(),
            status: match scanned {
                Some(profile) if string_field(profile, field) == *value => "verified",
                _ => "unconfirmed",
            }
            .to_string(),
        })
        .collect()
}

#[tauri::command]
pub async fn local_agent_profile_proxy_operation(
    client: State<'_, LocalAgentClient>,
    args: ProfileProxyOperationArgs,
) -> Result<ProfileProxyOperationResult, GuardRefusal> {
    let bit_profile_id = args.bit_profile_id.trim().to_string();
    if bit_profile_id.is_empty() {
        return Err(profile_refusal(
            "profile_id_required",
            "代理操作失败：缺少BitBrowser窗口ID",
        ));
    }
    let action = args.action.trim().to_ascii_lowercase();
    let payload = match action.as_str() {
        "apply" => {
            if args.host.trim().is_empty() || args.port.unwrap_or(0) <= 0 {
                return Err(profile_refusal(
                    "proxy_input_invalid",
                    "代理操作失败：缺少代理地址或端口",
                ));
            }
            serde_json::json!({
                "operation": "assign",
                "profile_id": bit_profile_id,
                "proxy_protocol": args.proxy_protocol,
                "host": args.host,
                "port": args.port,
                "username": args.username,
                "password": args.password,
            })
        }
        "unbind" => serde_json::json!({
            "operation": "unbind",
            "profile_id": bit_profile_id,
        }),
        _ => {
            return Err(profile_refusal(
                "proxy_action_invalid",
                "代理操作只支持 apply 或 unbind",
            ))
        }
    };
    let resp = client
        .post(if args.verify {
            "/api/v1/proxy-verification"
        } else {
            "/api/v1/proxy-write"
        })
        .json(&payload)
        .send()
        .await
        .map_err(GuardRefusal::unreachable)?;
    let status = resp.status();
    let text = resp.text().await.map_err(GuardRefusal::unreachable)?;
    if !status.is_success() {
        return Err(GuardRefusal::from_agent(status, &text));
    }
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|_| GuardRefusal::unreadable(status))?;
    let data = value
        .get("data")
        .ok_or_else(|| GuardRefusal::unreadable(status))?;
    let accepted = data
        .get("accepted")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let comparison = data
        .get("comparison")
        .and_then(|v| v.as_str())
        .filter(|v| ["matches", "different", "unverifiable"].contains(v))
        .map(String::from);
    if (args.verify && comparison.is_none()) || (!args.verify && !accepted) {
        return Err(profile_refusal(
            "proxy_result_unconfirmed",
            "无法确认比特代理操作结果，请先核实实际配置",
        ));
    }
    Ok(ProfileProxyOperationResult {
        action,
        bit_profile_id,
        readback: comparison.as_deref() == Some("matches"),
        accepted,
        comparison,
    })
}

/// Fetch a supplier's dynamic address from the operator's own network.
/// The supplier URL is supplied by the current Desktop flow. This command
/// returns the parsed connection without changing Cloud or BitBrowser state.
#[tauri::command]
pub async fn local_agent_proxy_extract(
    client: State<'_, LocalAgentClient>,
    args: ProxyExtractArgs,
) -> Result<ProxyExtractResult, GuardRefusal> {
    extract_proxy(&client, args).await
}

async fn extract_proxy(
    client: &LocalAgentClient,
    args: ProxyExtractArgs,
) -> Result<ProxyExtractResult, GuardRefusal> {
    if args.extract_url.trim().is_empty()
        || !["http", "https", "socks5"].contains(&args.proxy_protocol.as_str())
    {
        return Err(profile_refusal(
            "proxy_extract_input_invalid",
            "请填写有效的提取链接和代理协议",
        ));
    }
    let resp = client.post("/api/v1/proxy-extract")
        .json(&serde_json::json!({"extract_url": args.extract_url.trim(), "proxy_protocol": args.proxy_protocol}))
        .send().await.map_err(GuardRefusal::unreachable)?;
    let status = resp.status();
    let text = resp.text().await.map_err(GuardRefusal::unreachable)?;
    if !status.is_success() {
        return Err(GuardRefusal::from_agent(status, &text));
    }
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|_| GuardRefusal::unreadable(status))?;
    let data = value
        .get("data")
        .ok_or_else(|| GuardRefusal::unreadable(status))?;
    let result: ProxyExtractResult =
        serde_json::from_value(data.clone()).map_err(|_| GuardRefusal::unreadable(status))?;
    if result.host.trim().is_empty()
        || !(1..=65535).contains(&result.port)
        || !["http", "https", "socks5"].contains(&result.proxy_protocol.as_str())
    {
        return Err(profile_refusal(
            "proxy_extract_response_invalid",
            "提取结果缺少有效的代理地址",
        ));
    }
    Ok(result)
}

/// Restore selected Cloud profile facts to BitBrowser through Local Agent and
/// read back the full profile snapshot before reporting success.
#[tauri::command]
pub async fn local_agent_profile_restore(
    client: State<'_, LocalAgentClient>,
    profiles: Vec<RestoreProfileInput>,
) -> Result<RestoreProfileResult, GuardRefusal> {
    if profiles.is_empty() {
        return Err(profile_refusal(
            "profile_input_invalid",
            "没有可恢复的Cloud窗口配置",
        ));
    }
    for profile in &profiles {
        if profile.bit_profile_id.trim().is_empty() {
            return Err(profile_refusal(
                "profile_input_invalid",
                "恢复失败：存在缺少BitBrowser窗口ID的Cloud窗口",
            ));
        }
        let update_payload = restore_payload(profile);
        let update_resp = client
            .post("/api/v1/bit-browser/profile-update")
            .json(&update_payload)
            .send()
            .await
            .map_err(GuardRefusal::unreachable)?;
        if !update_resp.status().is_success() {
            let status = update_resp.status();
            let text = update_resp
                .text()
                .await
                .map_err(GuardRefusal::unreachable)?;
            return Err(GuardRefusal::from_agent(status, &text));
        }
    }

    let snapshot = local_agent_profile_scan(client.clone()).await?;
    verify_restored_profiles(&profiles, &snapshot)
        .map_err(|message| profile_refusal("profile_readback_unverified", message))?;
    Ok(RestoreProfileResult {
        restored_count: profiles.len(),
        profiles: profiles
            .iter()
            .map(|profile| RestoreProfileVerification {
                bit_profile_id: profile.bit_profile_id.clone(),
                status: "verified".into(),
            })
            .collect(),
        snapshot,
    })
}
fn restore_payload(profile: &RestoreProfileInput) -> serde_json::Value {
    let mut payload = serde_json::Map::new();
    payload.insert(
        "id".into(),
        serde_json::Value::String(profile.bit_profile_id.clone()),
    );
    if !profile.name.trim().is_empty() {
        payload.insert(
            "name".into(),
            serde_json::Value::String(profile.name.clone()),
        );
    }
    if let Some(seq) = profile.seq {
        payload.insert("seq".into(), serde_json::Value::Number(seq.into()));
    }
    if !profile.group_id.trim().is_empty() {
        payload.insert(
            "groupId".into(),
            serde_json::Value::String(profile.group_id.clone()),
        );
    }
    if !profile.group_name.trim().is_empty() {
        payload.insert(
            "groupName".into(),
            serde_json::Value::String(profile.group_name.clone()),
        );
    }
    if !profile.remark.trim().is_empty() {
        payload.insert(
            "remark".into(),
            serde_json::Value::String(profile.remark.clone()),
        );
    }
    serde_json::Value::Object(payload)
}
async fn post_local_agent_profile_operation(
    client: &LocalAgentClient,
    operation: &str,
    bit_profile_id: &str,
) -> Result<(), GuardRefusal> {
    let resp = client
        .post(&format!("/api/v1/bit-browser/{}", operation))
        .json(&serde_json::json!({"id": bit_profile_id}))
        .send()
        .await
        .map_err(GuardRefusal::unreachable)?;
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    let text = resp.text().await.map_err(GuardRefusal::unreachable)?;
    Err(GuardRefusal::from_agent(status, &text))
}
fn profile_refusal(code: &str, message: impl Into<String>) -> GuardRefusal {
    GuardRefusal {
        code: code.into(),
        message: message.into(),
        ..GuardRefusal::default()
    }
}
fn create_profile_payload(args: &CreateProfileArgs) -> Result<serde_json::Value, String> {
    let name = args.name.trim();
    let group_id = args.group_id.trim();
    if name.is_empty() {
        return Err("创建窗口失败：请填写窗口名称".into());
    }
    if group_id.is_empty() {
        return Err("创建窗口失败：请选择BitBrowser真实分组".into());
    }
    let mut payload = serde_json::Map::new();
    if !args.creation_marker.is_empty() {
        payload.insert(
            "creation_marker".into(),
            serde_json::json!(args.creation_marker),
        );
        payload.insert("inspect_only".into(), serde_json::json!(args.inspect_only));
    }
    payload.insert("name".into(), serde_json::Value::String(name.to_string()));
    payload.insert(
        "groupId".into(),
        serde_json::Value::String(group_id.to_string()),
    );
    if !args.group_name.trim().is_empty() {
        payload.insert(
            "groupName".into(),
            serde_json::Value::String(args.group_name.trim().to_string()),
        );
    }
    if let Some(seq) = args.seq {
        payload.insert("seq".into(), serde_json::Value::Number(seq.into()));
    }
    if !args.remark.trim().is_empty() {
        payload.insert(
            "remark".into(),
            serde_json::Value::String(args.remark.trim().to_string()),
        );
    }
    Ok(serde_json::Value::Object(payload))
}
fn verify_snapshot_has_profile(
    snapshot: &serde_json::Value,
    bit_profile_id: &str,
) -> Result<(), String> {
    let profiles = snapshot
        .get("profiles")
        .and_then(|value| value.as_array())
        .ok_or_else(|| "创建后读回BitBrowser结果格式不正确".to_string())?;
    if profiles
        .iter()
        .any(|profile| string_field(profile, "bit_profile_id") == bit_profile_id)
    {
        return Ok(());
    }
    Err(format!(
        "创建结果待确认：读回BitBrowser时没有找到新窗口 {}",
        bit_profile_id
    ))
}
fn verify_restored_profiles(
    expected: &[RestoreProfileInput],
    snapshot: &serde_json::Value,
) -> Result<(), String> {
    let scanned = snapshot
        .get("profiles")
        .and_then(|value| value.as_array())
        .ok_or_else(|| "读回BitBrowser结果格式不正确，无法确认恢复结果".to_string())?;
    for profile in expected {
        let actual = scanned
            .iter()
            .find(|item| string_field(item, "bit_profile_id") == profile.bit_profile_id)
            .ok_or_else(|| {
                format!(
                    "恢复结果待确认：读回时没有找到窗口 {}",
                    profile.bit_profile_id
                )
            })?;
        verify_string_field(
            actual,
            "name",
            &profile.name,
            "名称",
            &profile.bit_profile_id,
        )?;
        verify_string_field(
            actual,
            "group_id",
            &profile.group_id,
            "分组ID",
            &profile.bit_profile_id,
        )?;
        verify_string_field(
            actual,
            "group_name",
            &profile.group_name,
            "分组名称",
            &profile.bit_profile_id,
        )?;
    }
    Ok(())
}
fn verify_string_field(
    actual: &serde_json::Value,
    field: &str,
    expected: &str,
    label: &str,
    bit_profile_id: &str,
) -> Result<(), String> {
    if expected.trim().is_empty() {
        return Ok(());
    }
    let actual_value = string_field(actual, field);
    if actual_value != expected {
        return Err(format!(
            "恢复结果待确认：窗口 {} 的{}读回不一致",
            bit_profile_id, label
        ));
    }
    Ok(())
}
fn string_field(value: &serde_json::Value, field: &str) -> String {
    value
        .get(field)
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::testing::{arrivals, serving};
    use crate::config::{load_with, Environment, PRODUCTION_TOML};
    use crate::token::RuntimeToken;
    use std::collections::BTreeMap;

    #[test]
    fn dynamic_proxy_extraction_uses_local_agent_and_checks_response() {
        let (port, seen) = serving(|_| {
            (
                "200 OK".into(),
                r#"{"data":{"proxy_protocol":"socks5","host":"203.0.113.9","port":1080}}"#.into(),
            )
        });
        let config = load_with(
            &BTreeMap::new(),
            &PRODUCTION_TOML.replace("port = 8765", &format!("port = {port}")),
            Environment::Production,
        )
        .expect("test config");
        let client = LocalAgentClient::new(&config, RuntimeToken::generate());
        let result = tauri::async_runtime::block_on(extract_proxy(
            &client,
            ProxyExtractArgs {
                extract_url: "https://supplier.example/extract".into(),
                proxy_protocol: "socks5".into(),
            },
        ))
        .expect("local extraction");
        assert_eq!(result.host, "203.0.113.9");
        assert_eq!(result.port, 1080);
        assert_eq!(arrivals(&seen, "/api/v1/proxy-extract"), 1);
    }

    #[test]
    fn an_offline_bitbrowser_keeps_its_code_without_exposing_the_agent_diagnostic() {
        let (port, seen) = serving(|_| {
            (
                "503 Service Unavailable".into(),
                r#"{"error":{"code":"bit_browser_offline","message":"比特浏览器未运行或无法连接","retryable":true,"recovery_action":"start_bitbrowser"}}"#.into(),
            )
        });
        let config = load_with(
            &BTreeMap::new(),
            &PRODUCTION_TOML.replace("port = 8765", &format!("port = {port}")),
            Environment::Production,
        )
        .expect("test config");
        let client = LocalAgentClient::new(&config, RuntimeToken::generate());

        let error = tauri::async_runtime::block_on(post_local_agent_profile_operation(
            &client,
            "profile-open",
            "profile-1",
        ))
        .unwrap_err();

        assert_eq!(error.code, "bit_browser_offline");
        assert_eq!(error.recovery_action, "start_bitbrowser");
        assert!(!error.message.contains("503"));
        assert_eq!(arrivals(&seen, "/api/v1/bit-browser/profile-open"), 1);
    }

    fn restore_input() -> RestoreProfileInput {
        RestoreProfileInput {
            bit_profile_id: "profile-1".into(),
            name: "窗口一".into(),
            seq: Some(1),
            group_id: "group-1".into(),
            group_name: "分组一".into(),
            remark: "Cloud备注".into(),
        }
    }
    #[test]
    fn restore_payload_only_contains_safe_profile_fields() {
        let payload = restore_payload(&restore_input());

        assert_eq!(payload["id"], "profile-1");
        assert_eq!(payload["name"], "窗口一");
        assert_eq!(payload["groupId"], "group-1");
        assert_eq!(payload["groupName"], "分组一");
        assert!(payload.get("proxyType").is_none());
        assert!(payload.get("proxyHost").is_none());
        assert!(payload.get("proxyPort").is_none());
        assert!(payload.get("cookie").is_none());
        assert!(payload.get("user_id").is_none());
    }
    #[test]
    fn marked_create_recovery_serializes_inspect_only() {
        let args: CreateProfileArgs=serde_json::from_value(serde_json::json!({"name":"测试","group_id":"g","creation_marker":"WT_IMPORT:12:3:1","inspect_only":true})).unwrap();
        let payload = create_profile_payload(&args).unwrap();
        assert_eq!(payload["creation_marker"], "WT_IMPORT:12:3:1");
        assert_eq!(payload["inspect_only"], true);
        assert!(payload.get("input_cookie").is_none());
    }
    #[test]
    fn create_profile_payload_requires_real_group() {
        let err = create_profile_payload(&CreateProfileArgs {
            creation_marker: String::new(),
            inspect_only: false,
            name: "窗口一".into(),
            group_id: "".into(),
            group_name: "".into(),
            seq: None,
            remark: "".into(),
        })
        .unwrap_err();

        assert!(err.contains("真实分组"));
    }
    #[test]
    fn create_profile_payload_maps_safe_bitbrowser_fields() {
        let payload = create_profile_payload(&CreateProfileArgs {
            creation_marker: String::new(),
            inspect_only: false,
            name: "窗口一".into(),
            group_id: "group-1".into(),
            group_name: "默认分组".into(),
            seq: Some(7),
            remark: "备注".into(),
        })
        .unwrap();

        assert_eq!(payload["name"], "窗口一");
        assert_eq!(payload["groupId"], "group-1");
        assert_eq!(payload["groupName"], "默认分组");
        assert_eq!(payload["seq"], 7);
        assert_eq!(payload["remark"], "备注");
        assert!(payload.get("cookie").is_none());
        assert!(payload.get("user_id").is_none());
    }
    #[test]
    fn update_profile_payload_preserves_an_ungrouped_window() {
        let payload = update_profile_payload(&UpdateProfileArgs {
            bit_profile_id: "profile-1".into(),
            name: "窗口一".into(),
            group_id: "".into(),
            group_name: "".into(),
            remark: "新备注".into(),
        });

        assert_eq!(payload["remark"], "新备注");
        assert!(payload.get("groupId").is_none());
        assert!(payload.get("groupName").is_none());
    }
    #[test]
    fn verify_snapshot_has_profile_rejects_missing_created_profile() {
        let snapshot = serde_json::json!({
            "profiles": [{"bit_profile_id": "another"}]
        });

        let err = verify_snapshot_has_profile(&snapshot, "profile-created").unwrap_err();

        assert!(err.contains("没有找到新窗口"));
    }
    #[test]
    fn verify_restored_profiles_accepts_matching_readback() {
        let expected = vec![restore_input()];
        let snapshot = serde_json::json!({
            "main_user_id": "main-1",
            "profiles": [{
                "bit_profile_id": "profile-1",
                "name": "窗口一",
                "group_id": "group-1",
                "group_name": "分组一",
            }]
        });

        assert!(verify_restored_profiles(&expected, &snapshot).is_ok());
    }
    #[test]
    fn verify_restored_profiles_rejects_mismatched_readback() {
        let expected = vec![restore_input()];
        let snapshot = serde_json::json!({
            "profiles": [{
                "bit_profile_id": "profile-1",
                "name": "另一个名称",
                "group_id": "group-1",
                "group_name": "分组一",
            }]
        });

        let err = verify_restored_profiles(&expected, &snapshot).unwrap_err();
        assert!(err.contains("读回不一致"));
    }
}
