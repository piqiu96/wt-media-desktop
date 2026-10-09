//! One launch's system-provided path inputs.
//!
//! Every installed path starts from a *system* location, never from the
//! executable or the process working directory. macOS continues to derive the
//! `Library` layout from `HOME`; Windows derives the per-user tree from
//! `LOCALAPPDATA`. Holding the inputs together lets every consumer see the same
//! launch decision instead of consulting the environment separately.

use std::env;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const APPLICATION_DIR: &str = "WTMedia";
pub const DESKTOP_COMPONENT_DIR: &str = "Desktop";
pub const AGENT_COMPONENT_DIR: &str = "Agent";
pub const LOG_FALLBACK_DIR: &str = "wt-media-desktop-logs";

/// The platform whose installed layout this launch uses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Platform {
    Windows,
    Darwin,
    Unix,
}

/// The system inputs one launch resolves once.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystemPaths {
    pub platform: Platform,
    pub home: Option<PathBuf>,
    pub local_app_data: Option<PathBuf>,
    pub temp_dir: PathBuf,
}

/// Why a system path could not be resolved.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SystemPathError {
    NoHome,
    NoLocalAppData,
}

impl fmt::Display for SystemPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoHome => {
                write!(
                    f,
                    "HOME is not set, so the installed layout has no directory"
                )
            }
            Self::NoLocalAppData => write!(
                f,
                "LOCALAPPDATA is not set, so the Windows user layout has no directory"
            ),
        }
    }
}

impl std::error::Error for SystemPathError {}

static SYSTEM_PATHS: OnceLock<SystemPaths> = OnceLock::new();

impl SystemPaths {
    pub fn from_parts(
        platform: Platform,
        home: Option<PathBuf>,
        local_app_data: Option<PathBuf>,
        temp_dir: PathBuf,
    ) -> Self {
        Self {
            platform,
            home,
            local_app_data,
            temp_dir,
        }
    }

    /// The directory from which this platform's installed layout is built.
    pub fn production_base(&self) -> Result<&Path, SystemPathError> {
        match self.platform {
            Platform::Windows => self
                .local_app_data
                .as_deref()
                .ok_or(SystemPathError::NoLocalAppData),
            Platform::Darwin | Platform::Unix => {
                self.home.as_deref().ok_or(SystemPathError::NoHome)
            }
        }
    }

    /// The Agent's installed data root for the same platform.
    pub fn agent_default_data_dir(&self) -> Result<PathBuf, SystemPathError> {
        let base = self.production_base()?;
        Ok(match self.platform {
            Platform::Windows => base.join(APPLICATION_DIR).join(AGENT_COMPONENT_DIR),
            Platform::Darwin | Platform::Unix => base
                .join("Library")
                .join("Application Support")
                .join(APPLICATION_DIR)
                .join(AGENT_COMPONENT_DIR),
        })
    }

    /// The data root the first Windows release used before its per-user root
    /// gained the explicit `data` level.
    pub fn legacy_windows_desktop_data_dir(&self) -> Result<PathBuf, SystemPathError> {
        Ok(self
            .production_base()?
            .join(APPLICATION_DIR)
            .join(DESKTOP_COMPONENT_DIR))
    }

    /// The only fallback when the normal system root cannot be resolved.
    pub fn log_fallback(&self) -> PathBuf {
        self.temp_dir.join(LOG_FALLBACK_DIR)
    }

    /// Read the launch environment once. Tests should use `from_parts` instead.
    pub fn from_current_env() -> Self {
        Self::from_parts(
            Self::current_platform(),
            env::var_os("HOME").map(PathBuf::from),
            env::var_os("LOCALAPPDATA").map(PathBuf::from),
            env::temp_dir(),
        )
    }

    /// Install the launch value and return it. Later calls get the same value.
    pub fn init_from_env() -> Self {
        let paths = Self::from_current_env();
        let _ = SYSTEM_PATHS.set(paths.clone());
        paths
    }

    /// The value initialized at startup, or a best-effort value before that.
    pub fn current() -> Self {
        SYSTEM_PATHS
            .get()
            .cloned()
            .unwrap_or_else(Self::from_current_env)
    }

    pub const fn current_platform() -> Platform {
        if cfg!(target_os = "windows") {
            Platform::Windows
        } else if cfg!(target_os = "macos") {
            Platform::Darwin
        } else {
            Platform::Unix
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migration::{migrate_windows_legacy_data, MigrationReport};

    fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "wt-media-system-paths-{}-{}-{}",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::remove_dir_all(&path).ok();
        path
    }

    fn fixture(label: &str) -> PathBuf {
        let path = scratch(label);
        std::fs::create_dir_all(&path).expect("scratch root");
        path
    }

    fn plant(root: &Path, relative: &str, body: &[u8]) -> PathBuf {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("scratch tree");
        std::fs::write(&path, body).expect("scratch file");
        path
    }

    fn read(root: &Path, relative: &str) -> Vec<u8> {
        std::fs::read(root.join(relative)).expect("scratch file contents")
    }

    fn paths(platform: Platform, home: Option<&str>, local: Option<&str>) -> SystemPaths {
        SystemPaths::from_parts(
            platform,
            home.map(PathBuf::from),
            local.map(PathBuf::from),
            PathBuf::from("/tmp"),
        )
    }

    #[test]
    fn windows_production_base_is_local_app_data() {
        let paths = paths(Platform::Windows, Some("/home"), Some("/local"));
        let got = paths.production_base().expect("local app data");
        assert_eq!(got, Path::new("/local"));
    }

    #[test]
    fn macos_production_base_is_home() {
        let paths = paths(Platform::Darwin, Some("/home"), Some("/local"));
        let got = paths.production_base().expect("home");
        assert_eq!(got, Path::new("/home"));
    }

    #[test]
    fn missing_windows_local_app_data_is_an_error() {
        assert_eq!(
            paths(Platform::Windows, Some("/home"), None).production_base(),
            Err(SystemPathError::NoLocalAppData)
        );
    }

    #[test]
    fn missing_macos_home_is_an_error() {
        assert_eq!(
            paths(Platform::Darwin, None, Some("/local")).production_base(),
            Err(SystemPathError::NoHome)
        );
    }

    #[test]
    fn windows_agent_default_is_under_wt_media() {
        let got = paths(Platform::Windows, Some("/home"), Some("/local"))
            .agent_default_data_dir()
            .expect("agent default");
        assert_eq!(got, PathBuf::from("/local/WTMedia/Agent"));
    }

    #[test]
    fn macos_agent_default_keeps_the_library_layout() {
        let got = paths(Platform::Darwin, Some("/home"), Some("/local"))
            .agent_default_data_dir()
            .expect("agent default");
        assert_eq!(
            got,
            PathBuf::from("/home/Library/Application Support/WTMedia/Agent")
        );
    }

    #[test]
    fn log_fallback_is_an_explicit_temporary_tree() {
        assert_eq!(
            paths(Platform::Windows, None, None).log_fallback(),
            PathBuf::from("/tmp/wt-media-desktop-logs")
        );
    }

    #[test]
    fn absent_legacy_data_is_not_migrated() {
        let root = fixture("absent");
        let old = root.join("old");
        let new = root.join("new");

        assert_eq!(
            migrate_windows_legacy_data(&old, &new).expect("migration"),
            None
        );
        assert!(!new.exists());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn legacy_data_is_copied_into_an_empty_new_root_without_deletion() {
        let root = fixture("copy");
        let old = root.join("old");
        let new = root.join("new");
        plant(&old, "settings.toml", b"chosen");
        plant(&old, "nested/task.json", b"{}");
        std::fs::create_dir_all(&new).expect("empty new root");

        assert_eq!(
            migrate_windows_legacy_data(&old, &new).expect("migration"),
            Some(MigrationReport {
                copied_files: 2,
                skipped_files: 0,
            })
        );
        assert_eq!(read(&new, "settings.toml"), b"chosen");
        assert_eq!(read(&new, "nested/task.json"), b"{}");
        assert_eq!(read(&old, "settings.toml"), b"chosen");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn non_empty_new_data_stops_the_migration() {
        let root = fixture("occupied");
        let old = root.join("old");
        let new = root.join("new");
        plant(&old, "settings.toml", b"old");
        plant(&new, "settings.toml", b"new");

        assert_eq!(
            migrate_windows_legacy_data(&old, &new).expect("migration"),
            None
        );
        assert_eq!(read(&new, "settings.toml"), b"new");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn destination_collisions_are_never_overwritten() {
        let root = fixture("collision");
        let old = root.join("old");
        let new = root.join("new");
        plant(&old, "kept.bin", b"old");
        plant(&old, "copied.bin", b"copied");
        std::fs::create_dir_all(&new).expect("empty new root");
        std::fs::write(new.join("kept.bin"), b"new").expect("collision");

        let mut report = MigrationReport {
            copied_files: 0,
            skipped_files: 0,
        };
        crate::migration::copy_tree_if_absent(&old, &new, &mut report).expect("copy tree");
        assert_eq!(
            report,
            MigrationReport {
                copied_files: 1,
                skipped_files: 1,
            }
        );
        assert_eq!(read(&new, "kept.bin"), b"new");
        assert_eq!(read(&new, "copied.bin"), b"copied");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn agent_data_outside_the_desktop_roots_is_not_touched() {
        let root = fixture("agent-boundary");
        let old = root.join("desktop-old");
        let new = root.join("desktop-new");
        let agent = root.join("WTMedia").join("Agent");
        plant(&agent, "local-agent.sqlite3", b"agent");
        std::fs::create_dir_all(&new).expect("empty Desktop root");

        assert!(migrate_windows_legacy_data(&old, &new).is_ok());
        assert_eq!(read(&agent, "local-agent.sqlite3"), b"agent");
        std::fs::remove_dir_all(&root).ok();
    }
}
