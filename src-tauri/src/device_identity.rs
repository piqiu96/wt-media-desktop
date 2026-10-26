//! A display-only identifier for the computer running Desktop. It never enters
//! authentication, Agent requests, Cloud requests, or the diagnostic bundle.

use sha2::{Digest, Sha256};

pub fn display_id() -> Result<String, String> {
    let machine_id = read_machine_id().ok_or("无法读取系统机器标识")?;
    device_id_from_machine_id(&machine_id)
}

fn device_id_from_machine_id(machine_id: &str) -> Result<String, String> {
    let normalized = machine_id.trim().to_ascii_lowercase();
    if normalized.len() < 8 || !normalized.is_ascii() {
        return Err("系统机器标识无效".to_string());
    }
    let mut hash = Sha256::new();
    hash.update(b"wt-media-desktop-display-device-id-v1\0");
    hash.update(normalized.as_bytes());
    let digest = hash.finalize();
    Ok(format!("WT-{}", hex::encode(&digest[..16])))
}

#[cfg(any(target_os = "macos", test))]
fn extract_macos_machine_id(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let line = line.trim();
        if !line.starts_with("\"IOPlatformUUID\"") {
            return None;
        }
        line.split('"').nth(3).map(str::to_owned)
    })
}

#[cfg(any(target_os = "windows", test))]
fn extract_windows_machine_id(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() == 3
            && fields[0].eq_ignore_ascii_case("MachineGuid")
            && fields[1].eq_ignore_ascii_case("REG_SZ")
        {
            Some(fields[2].to_string())
        } else {
            None
        }
    })
}

#[cfg(target_os = "macos")]
fn read_machine_id() -> Option<String> {
    let output = std::process::Command::new("/usr/sbin/ioreg")
        .args(["-rd1", "-c", "IOPlatformExpertDevice"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    extract_macos_machine_id(&String::from_utf8(output.stdout).ok()?)
}

#[cfg(target_os = "windows")]
fn read_machine_id() -> Option<String> {
    let output = std::process::Command::new("reg.exe")
        .args([
            "query",
            r"HKLM\SOFTWARE\Microsoft\Cryptography",
            "/v",
            "MachineGuid",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    // `reg.exe` uses the local console encoding on some Windows installs. The
    // key name, type and GUID are ASCII even when surrounding text is not.
    extract_windows_machine_id(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(target_os = "linux")]
fn read_machine_id() -> Option<String> {
    ["/etc/machine-id", "/var/lib/dbus/machine-id"]
        .iter()
        .find_map(|path| std::fs::read_to_string(path).ok().filter(|id| !id.trim().is_empty()))
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn read_machine_id() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_id_is_stable_and_machine_specific() {
        let first = device_id_from_machine_id("machine-one").unwrap();
        assert_eq!(first, device_id_from_machine_id("machine-one").unwrap());
        assert_ne!(first, device_id_from_machine_id("machine-two").unwrap());
        assert!(first.starts_with("WT-"));
        assert!(!first.contains("machine-one"));
        assert!(device_id_from_machine_id(" ").is_err());
    }

    #[test]
    fn native_output_parsers_find_only_the_machine_id() {
        assert_eq!(
            extract_macos_machine_id("    \"IOPlatformUUID\" = \"FAKE-UUID-123\"\n"),
            Some("FAKE-UUID-123".to_string())
        );
        assert_eq!(
            extract_windows_machine_id("MachineGuid    REG_SZ    FAKE-GUID-456\r\n"),
            Some("FAKE-GUID-456".to_string())
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn installed_mac_has_a_repeatable_display_id() {
        let first = display_id().expect("macOS machine identifier is readable");
        assert_eq!(first, display_id().unwrap());
    }
}
