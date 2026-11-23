use crate::{
    app_paths,
    config::{load_with, Environment, PRODUCTION_TOML},
    sidecar,
    system_paths::{Platform, SystemPaths},
    token::RuntimeToken,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[test]
fn desktop_identity_does_not_follow_cloud_or_source_default() {
    let config = load_with(&BTreeMap::new(), PRODUCTION_TOML, Environment::Development).unwrap();
    assert_eq!(config.environment, Environment::Development);
    assert_eq!(config.cloud.cloud_env, "prod");
}

#[test]
fn all_desktop_environments_have_separate_system_roots() {
    let system = SystemPaths::from_parts(
        Platform::Darwin,
        Some(PathBuf::from("/home/operator")),
        None,
        PathBuf::from("/tmp"),
    );
    for (env, folder) in [
        (Environment::Development, "WTMedia/Dev"),
        (Environment::Test, "WTMedia/Test"),
        (Environment::Production, "WTMedia"),
    ] {
        let paths = app_paths::resolve(&system, env, Path::new("/checkout")).unwrap();
        assert_eq!(
            paths.data,
            PathBuf::from("/home/operator/Library/Application Support")
                .join(folder)
                .join("Desktop")
        );
        assert_eq!(
            paths.logs,
            PathBuf::from("/home/operator/Library/Logs")
                .join(folder)
                .join("Desktop")
        );
        assert!(!paths.cache.starts_with(&paths.data));
        let config = load_with(&BTreeMap::new(), PRODUCTION_TOML, env).unwrap();
        let vars: BTreeMap<_, _> =
            sidecar::environment(&config, &RuntimeToken::generate(), &system)
                .unwrap()
                .into_iter()
                .collect();
        let expected = PathBuf::from("/home/operator/Library/Application Support")
            .join(folder)
            .join("Agent");
        assert_eq!(
            vars.get("WT_MEDIA_AGENT_DATA_DIR"),
            Some(&expected.display().to_string())
        );
        assert_eq!(
            vars.get("WT_MEDIA_CLOUD_ENV").map(String::as_str),
            Some("prod")
        );
    }
}

#[test]
fn packaged_test_ignores_python_override_and_has_its_own_port() {
    let config = load_with(
        &BTreeMap::from([("WT_MEDIA_DESKTOP_ALLOW_PYTHON_FALLBACK".into(), "1".into())]),
        PRODUCTION_TOML,
        Environment::Test,
    )
    .unwrap();
    assert!(!config.development.python_fallback);
    assert_eq!(config.agent.port, 28765);
    assert_eq!(config.cloud.cloud_env, "prod");
}
#[test]
fn windows_and_custom_agent_roots_remain_scoped() {
    let system = SystemPaths::from_parts(
        Platform::Windows,
        None,
        Some(PathBuf::from("/local")),
        PathBuf::from("/tmp"),
    );
    for (env, folder) in [
        (Environment::Development, "WTMedia/Dev"),
        (Environment::Test, "WTMedia/Test"),
        (Environment::Production, "WTMedia"),
    ] {
        let config = load_with(&BTreeMap::new(), PRODUCTION_TOML, env).unwrap();
        let paths = app_paths::resolve(&system, env, Path::new("/checkout")).unwrap();
        assert_eq!(
            paths.data,
            PathBuf::from("/local").join(folder).join("Desktop/data")
        );
        let (data, logs) = app_paths::agent_directories(&config, &system).unwrap();
        assert_eq!(data, PathBuf::from("/local").join(folder).join("Agent"));
        assert_eq!(logs, data.join("logs"));
    }
    let mut config = load_with(&BTreeMap::new(), PRODUCTION_TOML, Environment::Test).unwrap();
    config.agent.data_dir = Some("/custom/agent".into());
    assert_eq!(
        app_paths::agent_directories(&config, &system).unwrap().0,
        PathBuf::from("/custom/agent/test")
    );
}
