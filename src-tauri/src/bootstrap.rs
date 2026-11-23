//! Native startup configuration. The compiled Desktop identity comes from
//! build_identity.rs; Cloud routing and CSP are loaded independently.
//! Corrupt resources retain the same compiled identity when falling back.

use crate::config::{self, DesktopConfig, Environment};
use crate::paths::{self, Source};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// What a launch starts from, and where each part came from.
#[derive(Debug)]
pub struct Startup {
    pub config: DesktopConfig,
    /// Which candidate supplied the text that was used.
    pub source: Source,
    /// Why a located file's own contents were rejected, if one was.
    ///
    /// Carries the message, which names the offending key and never its value
    /// (`config::parse_error` and `ConfigError::Invalid` both hold that line).
    pub rejected: Option<String>,
}

impl Startup {
    /// One line for stderr, naming the file that was read — or that none was.
    ///
    /// Before the window exists, stderr is the only channel there is, and
    /// "which config is this process actually running on" is the first question
    /// any diagnosis asks.
    pub fn summary(&self) -> String {
        let origin = match self.source {
            Source::EnvOverride => "环境变量指定的文件",
            Source::BundledResource => "安装包的资源目录",
            Source::BesideExecutable => "可执行文件旁",
            Source::DevelopmentTree => "开发树 resources/",
            Source::CompiledIn => "编译进二进制的默认值",
        };
        match &self.rejected {
            Some(reason) => format!("配置来自{origin}；另有一个文件被拒绝：{reason}"),
            None => format!("配置来自{origin}"),
        }
    }
}

/// The desktop identity embedded at build time, independent of Cloud routing.
pub fn build_environment() -> Environment {
    match env!("WT_MEDIA_DESKTOP_ENVIRONMENT") {
        "dev" => Environment::Development, "test" => Environment::Test,
        "prod" => Environment::Production, _ => panic!("invalid compiled desktop environment"),
    }
}

/// Locate the config, parse it, and fall back to the compiled-in default if the
/// located file turns out to be unusable.
///
/// The fallback is the same bargain [`paths::file_text`] makes for a file that
/// cannot be *read*: a launch with a known-good config beats no launch. "Corrupt"
/// is in that module's own list of cases, and a file that parses but fails
/// validation is the same problem one step later.
pub fn load(
    env: &BTreeMap<String, String>,
    environment: Environment,
    resource_dir: Option<&Path>,
    exe_dir: Option<&Path>,
    manifest_dir: &Path,
) -> Startup {
    let (text, source) = paths::file_text(env, environment, resource_dir, exe_dir, manifest_dir);

    match config::load_with(env, &text, environment) {
        Ok(config) => Startup {
            config,
            source,
            rejected: None,
        },
        Err(error) => Startup {
            config: compiled_default(env, environment),
            // The file was found and then not used, so reporting the source as
            // that file would be a lie in the one line an operator reads.
            source: Source::CompiledIn,
            rejected: Some(error.to_string()),
        },
    }
}

/// The compiled-in default, which cannot fail.
///
/// `expect` rather than a second fallback: there is nothing below this, and by
/// construction this branch is unreachable — `config::tests::
/// shipped_production_resource_parses_and_validates` and `paths::tests::
/// the_compiled_default_is_a_usable_config` both pin it. A panic before any
/// window exists is also the clearest possible failure if that ever changes.
fn compiled_default(env: &BTreeMap<String, String>, environment: Environment) -> DesktopConfig {
    config::load_with(env, config::PRODUCTION_TOML, environment).expect(
        "the compiled-in default must parse and validate; \
         config::tests::shipped_production_resource_parses_and_validates pins this",
    )
}

/// Resolve everything a launch needs from the real environment and filesystem.
///
/// The only impure part of this module, and deliberately thin: it gathers facts
/// and hands them to [`load`], which is where every decision lives.
pub fn resolve(package_info: &tauri::PackageInfo) -> Startup {
    // `std::env::vars()` panics on any entry that is not valid UTF-8, and an
    // environment is not ours to assume things about. Dropping such an entry is
    // safe here because the only two variables consulted are ones we name
    // ourselves.
    let env: BTreeMap<String, String> = std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect();

    let environment = build_environment();
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    // Exactly the call `PathResolver::resource_dir` makes, so the two cannot
    // drift — see this module's header.
    let resource_dir =
        tauri::utils::platform::resource_dir(package_info, &tauri::utils::Env::default()).ok();
    // The build machine's path, which is why it is only ever consulted when a
    // development tree is the layout in play.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    load(
        &env,
        environment,
        resource_dir.as_deref(),
        exe_dir.as_deref(),
        &manifest_dir,
    )
}

/// The CSP the window is served under, built from the config.
///
/// File capability URLs first reach Cloud and then redirect to object storage.
/// Allow that configured origin for images, including local HTTP deployments;
/// never interpolate a whole URL or the IPC/connect allowlist into img-src.
pub fn csp_policy(config: &DesktopConfig) -> String {
    let mut images = "'self' https:".to_string();
    if let Ok(url) = reqwest::Url::parse(&config.cloud.base_url) {
        if matches!(url.scheme(), "http" | "https") && url.has_host() {
            images.push(' ');
            images.push_str(&url.origin().ascii_serialization());
        }
    }
    format!(
        "default-src 'self'; connect-src 'self' {}; style-src 'self' 'unsafe-inline'; img-src {}; media-src 'self' {}",
        config.browser.csp_connect_src, images, config.browser.media_origins.join(" ")
    )
}

/// Put the policy on a Tauri config, in the field Tauri reads it from.
///
/// Takes the `Config` rather than the `Context` on purpose: the caller is one
/// line (`context.config_mut()`), and `Context` has no public constructor, so
/// taking it would make the mutation itself untestable while leaving the same
/// single line untested either way. This way only that line is.
///
/// Must run before `Builder::run`; see this module's header for why it cannot
/// run from `.setup()`.
pub fn apply_csp(target: &mut tauri::utils::config::Config, config: &DesktopConfig) {
    target.app.security.csp = Some(tauri::utils::config::Csp::Policy(csp_policy(config)));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// A directory with a `resources/desktop.production.toml` in it, removed by
    /// the caller.
    fn tree_with(text: &str, tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("wt-media-bootstrap-{tag}-{}", std::process::id()));
        let resources = dir.join("resources");
        std::fs::create_dir_all(&resources).expect("temp dir");
        std::fs::write(resources.join(paths::RESOURCE_NAME), text).expect("write config");
        dir
    }

    fn shipped() -> DesktopConfig {
        config::load_with(
            &BTreeMap::new(),
            config::PRODUCTION_TOML,
            Environment::Production,
        )
        .expect("the shipped resource must load")
    }

    #[test]
    fn startup_uses_the_compiled_desktop_identity() {
        assert_eq!(
            build_environment(),
            match env!("WT_MEDIA_DESKTOP_ENVIRONMENT") {
        "dev" => Environment::Development, "test" => Environment::Test,
        "prod" => Environment::Production, _ => panic!("invalid compiled desktop environment"),
    }
        );
    }

    /// `tauri.conf.json` must not carry a policy of its own any more.
    ///
    /// The two keys are not equally dangerous and the test says which is which:
    ///
    /// - `csp`: `apply_csp` overwrites it, so a literal coming back would still
    ///   be **dead** — it would not change behaviour, it would only mislead the
    ///   next reader into thinking that line is the policy.
    /// - `devCsp`: this one **wins**. `Manager::csp()` prefers `dev_csp` and
    ///   falls back to `csp` (`tauri/src/manager/mod.rs:369-380`), and `apply_csp`
    ///   sets only `csp` — so a `devCsp` literal would silently override the
    ///   injected policy in every dev build, and the file would be the thing in
    ///   force again.
    ///
    /// Together with `apply_csp_writes_the_field_a_dev_build_would_otherwise_
    /// prefer_over` (which pins `dev_csp` staying `None`), this is what makes
    /// "the injected policy is the policy, in dev and in production" hold.
    #[test]
    fn tauri_conf_carries_no_policy_of_its_own() {
        let conf: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json"))
            .expect("tauri.conf.json is JSON");
        let security = &conf["app"]["security"];

        for key in ["csp", "devCsp"] {
            assert!(
                security.get(key).is_none(),
                "tauri.conf.json must not declare {key}: the policy comes from the desktop \
                 config through `apply_csp`. Found {security:?}"
            );
        }
    }

    /// Keep every directive deliberate. Images additionally permit the configured
    /// Cloud origin so local HTTP capability URLs can redirect to the bucket.
    #[test]
    fn the_policy_is_shape_for_shape_the_literal_it_replaced() {
        assert_eq!(
            csp_policy(&shipped()),
            "default-src 'self'; connect-src 'self' ipc: http://ipc.localhost \
             http://127.0.0.1:8188; style-src 'self' 'unsafe-inline'; img-src 'self' https: http://127.0.0.1:8188; \
             media-src 'self' https://s3.oss.longyanyue.cn https://data.bucket.oss.longyanyue.cn"
        );
    }

    #[test]
    fn media_sources_follow_configuration_without_allowing_other_hosts() {
        let mut config = shipped();
        config.browser.media_origins = vec!["https://media.example.test".into()];
        let policy = csp_policy(&config);
        let media = policy.split(';').find(|d| d.trim().starts_with("media-src ")).unwrap();
        assert_eq!(media.trim(), "media-src 'self' https://media.example.test");
        assert!(!media.contains("longyanyue.cn"));
        assert!(!media.split_whitespace().any(|source| source == "https:" || source == "*"));
    }

    /// The mutation lands in `app.security.csp` — the field `Manager::csp()`
    /// reads. `dev_csp` is left alone deliberately: it takes precedence in a dev
    /// build, so setting it would silently override the policy with a second
    /// value the config file does not carry.
    #[test]
    fn apply_csp_writes_the_field_a_dev_build_would_otherwise_prefer_over() {
        let mut target = tauri::utils::config::Config::default();
        let config = shipped();

        apply_csp(&mut target, &config);

        let expected = tauri::utils::config::Csp::Policy(csp_policy(&config));
        assert_eq!(target.app.security.csp, Some(expected));
        assert_eq!(target.app.security.dev_csp, None);
    }

    /// connect-src follows its own config rather than the image origin.
    #[test]
    fn the_policy_takes_its_connect_src_from_the_config() {
        let mut config = shipped();
        config.browser.csp_connect_src = "https://cloud.example.test".to_string();

        let policy = csp_policy(&config);

        assert!(
            policy.contains("connect-src 'self' https://cloud.example.test;"),
            "{policy}"
        );
        assert!(
            !policy
                .split(';')
                .find(|d| d.contains("connect-src"))
                .unwrap()
                .contains("127.0.0.1:8188"),
            "connect-src must not retain the previous origin: {policy}"
        );
    }

    #[test]
    fn images_allow_the_configured_cloud_origin_without_exposing_url_details() {
        let mut config = shipped();
        config.cloud.base_url =
            "http://operator:secret@localhost:19088/cloud/api?key=private#ignored".into();
        let policy = csp_policy(&config);
        let images = policy
            .split(';')
            .find(|d| d.trim().starts_with("img-src "))
            .unwrap();
        assert_eq!(
            images.trim(),
            "img-src 'self' https: http://localhost:19088"
        );
        assert!(!images.contains("secret"));
        assert!(!images.contains("ipc.localhost"));
        assert!(!images
            .split_whitespace()
            .any(|source| source == "http:" || source == "*"));
    }

    #[test]
    fn images_reject_non_http_cloud_origins_and_policy_injection() {
        let mut config = shipped();
        config.cloud.base_url = "javascript:alert(1)".into();
        assert!(csp_policy(&config).contains("img-src 'self' https:;"));
        config.cloud.base_url = "http://cloud.example/api/;script-src *".into();
        let policy = csp_policy(&config);
        assert!(policy.contains("img-src 'self' https: http://cloud.example;"));
        assert!(!policy.contains("script-src *"));
    }

    /// A development build with a development file in the tree runs on that
    /// file, and says so.
    #[test]
    fn a_development_build_reads_the_tree_and_reports_it() {
        let text = config::PRODUCTION_TOML
            .replace(
                "environment = \"prod\"",
                "environment = \"dev\"",
            )
            .replace("port = 8765", "port = 8766");
        let dir = tree_with(&text, "dev");

        let startup = load(&BTreeMap::new(), Environment::Development, None, None, &dir);

        std::fs::remove_dir_all(&dir).ok();

        assert_eq!(startup.source, Source::DevelopmentTree);
        assert_eq!(startup.config.agent.port, 8766);
        assert!(startup.rejected.is_none());
        assert!(
            startup.summary().contains("开发树"),
            "{}",
            startup.summary()
        );
    }

    /// A file that parses but is not usable falls back to the compiled-in
    /// default instead of aborting the launch — and the reason is kept, by key.
    #[test]
    fn a_rejected_file_falls_back_and_says_why_without_echoing_a_value() {
        let text = config::PRODUCTION_TOML
            .replace(
                "environment = \"prod\"",
                "environment = \"dev\"",
            )
            .replace("host = \"127.0.0.1\"", "host = \"0.0.0.0\"");
        let dir = tree_with(&text, "bad");

        let startup = load(&BTreeMap::new(), Environment::Development, None, None, &dir);

        std::fs::remove_dir_all(&dir).ok();

        assert_eq!(
            startup.config.agent.port, 18765,
            "the fallback must be the shipped config"
        );
        assert_eq!(
            startup.source,
            Source::CompiledIn,
            "a rejected file is not the source"
        );
        let reason = startup.rejected.as_ref().expect("the reason must be kept");
        assert!(reason.contains("agent.host"), "{reason}");
        assert!(
            !reason.contains("0.0.0.0"),
            "the value must not be echoed: {reason}"
        );
        assert!(startup.summary().contains(reason), "{}", startup.summary());
    }

    /// Rule 1, end to end: in production, neither the locator nor the one
    /// development override has any effect, and `paths` is not what enforces
    /// that — both are ignored by two different modules and this is the test
    /// that sees them together.
    #[test]
    fn production_ignores_the_environment_end_to_end() {
        let text = config::PRODUCTION_TOML
            .replace(
                "environment = \"prod\"",
                "environment = \"dev\"",
            )
            .replace("port = 8765", "port = 9999");
        let dir = tree_with(&text, "prod");
        let env = env_of(&[
            (
                paths::CONFIG_ENV,
                dir.join("resources")
                    .join(paths::RESOURCE_NAME)
                    .to_str()
                    .unwrap(),
            ),
            (config::ENV_PYTHON_FALLBACK, "1"),
        ]);

        let startup = load(&env, Environment::Production, None, None, &dir);

        std::fs::remove_dir_all(&dir).ok();

        assert_ne!(startup.source, Source::EnvOverride, "{}", startup.summary());
        assert_eq!(startup.config.agent.port, 8765);
        assert!(!startup.config.development.python_fallback);
        assert_eq!(startup.config.environment, Environment::Production);
    }
}
