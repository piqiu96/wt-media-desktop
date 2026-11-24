mod build_identity;
use serde_json::{json, Value};
fn merge(target: &mut Value, patch: Value) {
    if let (Some(target), Some(patch)) = (target.as_object_mut(), patch.as_object()) {
        for (key, value) in patch {
            merge(target.entry(key).or_insert(Value::Null), value.clone());
        }
    } else {
        *target = patch;
    }
}
fn main() {
    for key in [
        "WT_MEDIA_DESKTOP_ENVIRONMENT",
        "WT_MEDIA_DESKTOP_RELEASE_CHANNEL",
        "TAURI_CONFIG",
        "WT_MEDIA_PRODUCT_TAG",
    ] {
        println!("cargo:rerun-if-env-changed={key}");
    }
    println!("cargo:rerun-if-changed=build_identity.rs");
    let profile = std::env::var("PROFILE").unwrap();
    let requested = std::env::var("WT_MEDIA_DESKTOP_ENVIRONMENT").ok();
    let channel = std::env::var("WT_MEDIA_DESKTOP_RELEASE_CHANNEL").ok();
    let environment = build_identity::resolve(&profile, requested.as_deref(), channel.as_deref())
        .expect("desktop build identity refused");
    let suffix = match environment {
        "dev" => ".dev",
        "test" => ".test",
        _ => "",
    };
    let name = match environment {
        "dev" => "起飞 Dev",
        "test" => "起飞 Test",
        _ => "起飞",
    };
    let identifier = format!("com.wtmedia.desktop{suffix}");
    let binary = format!(
        "wt-media-desktop{}",
        if environment == "prod" {
            "-shell"
        } else {
            suffix
        }
    );
    let mut overlay: Value =
        serde_json::from_str(&std::env::var("TAURI_CONFIG").unwrap_or_else(|_| "{}".into()))
            .expect("valid Tauri overlay");
    if let Some(value) = overlay.get("identifier") {
        assert_eq!(
            value.as_str(),
            Some(identifier.as_str()),
            "Tauri overlay and Desktop environment differ"
        );
    }
    if let Some(value) = overlay.get("productName") {
        assert_eq!(
            value.as_str(),
            Some(name),
            "Tauri product name and Desktop identity differ"
        );
    }
    // A non-production installer must use the matching CLI packaging overlay,
    // otherwise the native context and installer metadata could disagree.
    if profile == "release" && environment == "test" {
        assert_eq!(
            overlay.get("identifier").and_then(Value::as_str),
            Some(identifier.as_str()),
            "test installers must use scripts/build-desktop-environment.py"
        );
        assert_eq!(
            overlay.get("productName").and_then(Value::as_str),
            Some(name),
            "test installer product name required"
        );
        assert_eq!(
            overlay.get("mainBinaryName").and_then(Value::as_str),
            Some(binary.as_str()),
            "test installer binary name required"
        );
    }
    if let Ok(tag) = std::env::var("WT_MEDIA_PRODUCT_TAG") {
        assert_eq!(
            profile, "release",
            "product releases require release profile"
        );
        assert_eq!(
            environment, "prod",
            "product releases require prod identity"
        );
        assert!(tag.starts_with('v'), "invalid product tag");
        assert_eq!(
            overlay.get("version").and_then(Value::as_str),
            Some(&tag[1..]),
            "product version must reach Tauri CLI through its configuration overlay"
        );
    } else {
        assert!(
            channel.as_deref() != Some("stable"),
            "official release requires product tag"
        );
    }
    let base: Value =
        serde_json::from_str(&std::fs::read_to_string("tauri.conf.json").unwrap()).unwrap();
    if profile == "release" {
        assert!(overlay.get("version").and_then(Value::as_str).is_some(),
            "Desktop packages require the version overlay: use build-desktop-environment.py or the release entry point");
    } else if overlay.get("version").is_none() {
        // Direct `cargo tauri dev` is an unpublished build as well.
        overlay["version"] = json!(format!("{}-dev", base["version"].as_str().unwrap()));
    }
    let mut windows = base["app"]["windows"].clone();
    windows[0]["title"] = json!(format!("{name} · 内容运营平台"));
    let icons = if environment == "prod" {
        "icons".to_string()
    } else {
        format!("icons/{environment}")
    };
    merge(
        &mut overlay,
        json!({"productName":name,"identifier":identifier,"mainBinaryName":binary,"app":{"windows":windows},"bundle":{"icon":[format!("{icons}/icon.png"),format!("{icons}/icon.icns"),format!("{icons}/icon.ico")]}}),
    );
    let serialized = overlay.to_string();
    std::env::set_var("TAURI_CONFIG", &serialized);
    println!("cargo:rustc-env=TAURI_CONFIG={serialized}");
    println!("cargo:rustc-env=WT_MEDIA_DESKTOP_ENVIRONMENT={environment}");
    tauri_build::build();
}
