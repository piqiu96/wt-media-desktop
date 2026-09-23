//! BitBrowser profile commands: scan, groups, open/close, create and restore.
//!
//! Desktop owns this local bridge. The Vue layer must not call the Local Agent
//! port or hold Local Agent credentials directly.

use crate::dto::{
    CreateProfileArgs, CreateProfileResult, ProfileOperationArgs, ProfileOperationResult,
    RestoreProfileInput, RestoreProfileResult, RestoreProfileVerification,
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
) -> Result<serde_json::Value, String> {
    let resp = client
        .post("/api/v1/bit-browser/profile-scans")
        .send()
        .await
        .map_err(|e| format!("profile scan failed: {}", e))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("profile scan failed: {} {}", status, text));
    }
    resp.json()
        .await
        .map_err(|e| format!("invalid profile scan response: {}", e))
}
#[tauri::command]
pub async fn local_agent_profile_groups(
    client: State<'_, LocalAgentClient>,
) -> Result<serde_json::Value, String> {
    let resp = client
        .post("/api/v1/bit-browser/profile-groups")
        .send()
        .await
        .map_err(|e| format!("读取BitBrowser分组失败: {}", e))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("读取BitBrowser分组失败: {} {}", status, text));
    }
    resp.json()
        .await
        .map_err(|e| format!("BitBrowser分组响应格式错误: {}", e))
}
#[tauri::command]
pub async fn local_agent_profile_open(
    client: State<'_, LocalAgentClient>,
    args: ProfileOperationArgs,
) -> Result<ProfileOperationResult, String> {
    let bit_profile_id = args.bit_profile_id.trim().to_string();
    if bit_profile_id.is_empty() {
        return Err("打开窗口失败：缺少BitBrowser窗口ID".into());
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
) -> Result<ProfileOperationResult, String> {
    let bit_profile_id = args.bit_profile_id.trim().to_string();
    if bit_profile_id.is_empty() {
        return Err("关闭窗口失败：缺少BitBrowser窗口ID".into());
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
) -> Result<CreateProfileResult, String> {
    let payload = create_profile_payload(&args)?;
    let resp = client
        .post("/api/v1/bit-browser/profile-create")
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("创建BitBrowser窗口失败: {}", e))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("创建BitBrowser窗口失败: {} {}", status, text));
    }
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("创建BitBrowser窗口响应格式错误: {}", e))?;
    let bit_profile_id = value
        .get("data")
        .and_then(|data| data.get("id"))
        .and_then(|id| id.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    if bit_profile_id.is_empty() {
        return Err("创建BitBrowser窗口后未返回窗口ID".into());
    }
    let snapshot = local_agent_profile_scan(client.clone()).await?;
    verify_snapshot_has_profile(&snapshot, &bit_profile_id)?;
    Ok(CreateProfileResult {
        bit_profile_id,
        snapshot,
    })
}
/// Restore selected Cloud profile facts to BitBrowser through Local Agent and
/// read back the full profile snapshot before reporting success.
#[tauri::command]
pub async fn local_agent_profile_restore(
    client: State<'_, LocalAgentClient>,
    profiles: Vec<RestoreProfileInput>,
) -> Result<RestoreProfileResult, String> {
    if profiles.is_empty() {
        return Err("没有可恢复的Cloud窗口配置".into());
    }
    for profile in &profiles {
        if profile.bit_profile_id.trim().is_empty() {
            return Err("恢复失败：存在缺少BitBrowser窗口ID的Cloud窗口".into());
        }
        let update_payload = restore_payload(profile);
        let update_resp = client
            .post("/api/v1/bit-browser/profile-update")
            .json(&update_payload)
            .send()
            .await
            .map_err(|e| format!("写回BitBrowser失败: {}", e))?;
        if !update_resp.status().is_success() {
            let status = update_resp.status();
            let text = update_resp.text().await.unwrap_or_default();
            return Err(format!("写回BitBrowser失败: {} {}", status, text));
        }
    }

    let snapshot = local_agent_profile_scan(client.clone()).await?;
    verify_restored_profiles(&profiles, &snapshot)?;
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
    if !profile.proxy_type.trim().is_empty() {
        payload.insert(
            "proxyType".into(),
            serde_json::Value::String(profile.proxy_type.clone()),
        );
    }
    if !profile.proxy_host.trim().is_empty() {
        payload.insert(
            "proxyHost".into(),
            serde_json::Value::String(profile.proxy_host.clone()),
        );
    }
    if let Some(port) = profile.proxy_port {
        payload.insert("proxyPort".into(), serde_json::Value::Number(port.into()));
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
) -> Result<(), String> {
    let resp = client
        .post(&format!("/api/v1/bit-browser/{}", operation))
        .json(&serde_json::json!({"id": bit_profile_id}))
        .send()
        .await
        .map_err(|e| format!("BitBrowser窗口操作失败: {}", e))?;
    if resp.status().is_success() {
        return Ok(());
    }
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    Err(format!("BitBrowser窗口操作失败: {} {}", status, text))
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
        verify_string_field(
            actual,
            "proxy_type",
            &profile.proxy_type,
            "代理类型",
            &profile.bit_profile_id,
        )?;
        verify_string_field(
            actual,
            "proxy_host",
            &profile.proxy_host,
            "代理地址",
            &profile.bit_profile_id,
        )?;
        verify_i64_field(
            actual,
            "proxy_port",
            profile.proxy_port,
            "代理端口",
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
fn verify_i64_field(
    actual: &serde_json::Value,
    field: &str,
    expected: Option<i64>,
    label: &str,
    bit_profile_id: &str,
) -> Result<(), String> {
    let Some(expected_value) = expected else {
        return Ok(());
    };
    let actual_value = actual
        .get(field)
        .and_then(|value| value.as_i64())
        .unwrap_or_default();
    if actual_value != expected_value {
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

    fn restore_input() -> RestoreProfileInput {
        RestoreProfileInput {
            bit_profile_id: "profile-1".into(),
            name: "窗口一".into(),
            seq: Some(1),
            group_id: "group-1".into(),
            group_name: "分组一".into(),
            proxy_type: "socks5".into(),
            proxy_host: "127.0.0.1".into(),
            proxy_port: Some(1080),
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
        assert_eq!(payload["proxyType"], "socks5");
        assert_eq!(payload["proxyHost"], "127.0.0.1");
        assert_eq!(payload["proxyPort"], 1080);
        assert!(payload.get("cookie").is_none());
        assert!(payload.get("user_id").is_none());
    }
    #[test]
    fn create_profile_payload_requires_real_group() {
        let err = create_profile_payload(&CreateProfileArgs {
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
                "proxy_type": "socks5",
                "proxy_host": "127.0.0.1",
                "proxy_port": 1080
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
                "proxy_type": "socks5",
                "proxy_host": "127.0.0.1",
                "proxy_port": 1080
            }]
        });

        let err = verify_restored_profiles(&expected, &snapshot).unwrap_err();
        assert!(err.contains("读回不一致"));
    }
}
