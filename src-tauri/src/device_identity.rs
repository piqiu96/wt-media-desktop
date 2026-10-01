//! Stable installation identity for proving that this Desktop is the bound device.
//! The private key remains in native app data and never enters the WebView or Agent.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use ring::{rand::SystemRandom, signature::{Ed25519KeyPair, KeyPair}};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{fs::{self, OpenOptions}, io::Write, path::Path};
use tauri::Manager;

const KEY_FILE: &str = "device-identity.pk8";

pub struct DeviceIdentity {
    key: Ed25519KeyPair,
    pub device_id: String,
    pub device_name: String,
}

#[derive(Serialize)]
pub struct DeviceIdentitySummary {
    pub device_id: String,
    pub device_name: String,
}

impl DeviceIdentity {
    pub fn for_app(app: &tauri::AppHandle) -> Result<Self, String> {
        let directory = app.path().app_data_dir().map_err(|_| "无法定位本机设备信息目录")?;
        Self::load_or_create(&directory)
    }

    fn load_or_create(directory: &Path) -> Result<Self, String> {
        fs::create_dir_all(directory).map_err(|_| "无法创建本机设备信息目录")?;
        let path = directory.join(KEY_FILE);
        let pkcs8 = if path.exists() {
            fs::read(&path).map_err(|_| "无法读取本机设备身份，请检查应用数据目录")?
        } else {
            let rng = SystemRandom::new();
            let generated = Ed25519KeyPair::generate_pkcs8(&rng).map_err(|_| "无法生成本机设备身份")?;
            let temp = directory.join(format!("{KEY_FILE}.tmp"));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
            let mut file = options.open(&temp).map_err(|_| "无法安全写入本机设备身份")?;
            file.write_all(generated.as_ref()).map_err(|_| "无法写入本机设备身份")?;
            file.sync_all().map_err(|_| "无法保存本机设备身份")?;
            fs::rename(&temp, &path).map_err(|_| "无法完成本机设备身份保存")?;
            generated.as_ref().to_vec()
        };
        let key = Ed25519KeyPair::from_pkcs8(&pkcs8).map_err(|_| "本机设备身份文件已损坏，请在个人信息页解除旧绑定后修复")?;
        let device_id = hex::encode(Sha256::digest(key.public_key().as_ref()));
        let device_name = match std::env::consts::OS {
            "macos" => "macOS 运营电脑",
            "windows" => "Windows 运营电脑",
            _ => "运营电脑",
        }.to_string();
        Ok(Self { key, device_id, device_name })
    }

    pub fn summary(&self) -> DeviceIdentitySummary {
        DeviceIdentitySummary { device_id: self.device_id.clone(), device_name: self.device_name.clone() }
    }

    pub fn public_key(&self) -> String { URL_SAFE_NO_PAD.encode(self.key.public_key().as_ref()) }

    pub fn sign_ticket(&self, ticket: &str) -> String {
        let message = format!("wt-media-device-v1\n{ticket}\n{}", self.device_id);
        URL_SAFE_NO_PAD.encode(self.key.sign(message.as_bytes()).as_ref())
    }
}

#[tauri::command]
pub fn local_device_identity(app: tauri::AppHandle) -> Result<DeviceIdentitySummary, String> {
    Ok(DeviceIdentity::for_app(&app)?.summary())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{UnparsedPublicKey, ED25519};

    #[test]
    fn identity_survives_restart_and_signs_only_the_current_ticket() {
        let directory = std::env::temp_dir().join(format!("wt-media-device-{}", uuid::Uuid::new_v4()));
        let first = DeviceIdentity::load_or_create(&directory).unwrap();
        let second = DeviceIdentity::load_or_create(&directory).unwrap();
        assert_eq!(first.device_id, second.device_id);
        let signature = URL_SAFE_NO_PAD.decode(second.sign_ticket("ticket-a")).unwrap();
        let key = URL_SAFE_NO_PAD.decode(second.public_key()).unwrap();
        let message = format!("wt-media-device-v1\nticket-a\n{}", second.device_id);
        assert!(UnparsedPublicKey::new(&ED25519, key).verify(message.as_bytes(), &signature).is_ok());
        assert!(UnparsedPublicKey::new(&ED25519, URL_SAFE_NO_PAD.decode(second.public_key()).unwrap()).verify(b"wt-media-device-v1\nticket-b", &signature).is_err());
        fs::remove_dir_all(directory).unwrap();
    }
}
