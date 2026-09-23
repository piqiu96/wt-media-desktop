//! Turning the configuration file into what a launch needs, and the one step
//! that has to happen before Tauri builds anything.
//!
//! That step is the CSP, and where it may happen is narrower than it looks.
//! Tauri reads `app.security.csp` out of the **`AppManager`'s own copy** of the
//! config: `AppManager::with_handlers` moves `context.config` into the manager
//! (`tauri/src/manager/mod.rs:39`), and `Manager::csp()` reads it from there when
//! it serves an asset (`:369`). `App` exposes no `config_mut`, so once
//! `Builder::build` has run that copy is unreachable — and `.setup()` runs even
//! later than that, *after* Tauri has built every window declared in the config
//! file (`tauri/src/app.rs:2524` then `:2531`). So the mutation happens on the
//! `Context`, before `run()`, and the config has to be located without an
//! `AppHandle`.
//!
//! Locating it without an `AppHandle` costs nothing, because the resource
//! directory does not need one: `PathResolver::resource_dir` is a thin wrapper
//! over `tauri::utils::platform::resource_dir(package_info, env)`
//! (`tauri/src/path/desktop.rs`), and `Context::package_info()` is available
//! before `build`. Calling that same function here means the directory resolved
//! now and the one `app.path().resource_dir()` would return cannot drift.
//!
//! The environment comes from the **build**, never from the file: the file
//! cannot be located until we know where to look, and a release binary is
//! production with no way to be talked out of it.

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

/// The environment a build fixes, as a pure function of "is this a debug build".
///
/// Split from [`build_environment`] so both directions are testable — the same
/// shape as `python_fallback_allowed` in `main.rs`, and for the same reason: a
/// rule that can only ever be observed one way is not tested.
pub const fn build_environment_of(debug_build: bool) -> Environment {
    if debug_build {
        Environment::Development
    } else {
        Environment::Production
    }
}

/// The environment this binary is.
pub const fn build_environment() -> Environment {
    build_environment_of(cfg!(debug_assertions))
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
        Ok(config) => Startup { config, source, rejected: None },
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
/// The four directives are the ones `tauri.conf.json` carried as a literal; the
/// origin that is deployment-specific — Cloud's — is the one value taken from
/// the file. Keeping the rest fixed is deliberate: this function exists to move
/// *that* value out of the source tree, not to make the policy a second thing an
/// operator has to get right.
pub fn csp_policy(config: &DesktopConfig) -> String {
    format!(
        "default-src 'self'; connect-src 'self' {}; style-src 'self' 'unsafe-inline'; img-src 'self' https:",
        config.browser.csp_connect_src
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
        let dir = std::env::temp_dir().join(format!(
            "wt-media-bootstrap-{tag}-{}",
            std::process::id()
        ));
        let resources = dir.join("resources");
        std::fs::create_dir_all(&resources).expect("temp dir");
        std::fs::write(resources.join(paths::RESOURCE_NAME), text).expect("write config");
        dir
    }

    fn shipped() -> DesktopConfig {
        config::load_with(&BTreeMap::new(), config::PRODUCTION_TOML, Environment::Production)
            .expect("the shipped resource must load")
    }

    #[test]
    fn a_debug_build_is_development_and_a_release_build_is_production() {
        assert_eq!(build_environment_of(true), Environment::Development);
        assert_eq!(build_environment_of(false), Environment::Production);
        // And the wiring agrees with the rule in the profile this test runs in.
        assert_eq!(build_environment(), build_environment_of(cfg!(debug_assertions)));
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
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri.conf.json is JSON");
        let security = &conf["app"]["security"];

        for key in ["csp", "devCsp"] {
            assert!(
                security.get(key).is_none(),
                "tauri.conf.json must not declare {key}: the policy comes from the desktop \
                 config through `apply_csp`. Found {security:?}"
            );
        }
    }

    /// The policy's whole shape is pinned, not only its variable part.
    ///
    /// The string below is, deliberately, **the literal that used to sit in
    /// `tauri.conf.json`** — same directives, same order, same separators. Until
    /// that literal was deleted, `the_shipped_config_produces_the_policy_tauri_
    /// conf_carries` asserted this equality byte for byte against the file; when
    /// the file lost its copy, the assertion moved here rather than being
    /// dropped. Its job now is to make "the policy moved, it did not change" a
    /// permanent claim: a directive that is edited, added or reordered has to
    /// edit this string too, on purpose.
    #[test]
    fn the_policy_is_shape_for_shape_the_literal_it_replaced() {
        assert_eq!(
            csp_policy(&shipped()),
            "default-src 'self'; connect-src 'self' http://127.0.0.1:18080; \
             style-src 'self' 'unsafe-inline'; img-src 'self' https:"
        );
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

    /// And the one directive that is allowed to move is the one that reads the
    /// config — otherwise the test above would also pass for a function that
    /// ignores its argument.
    #[test]
    fn the_policy_takes_its_connect_src_from_the_config() {
        let mut config = shipped();
        config.browser.csp_connect_src = "https://cloud.example.test".to_string();

        let policy = csp_policy(&config);

        assert!(
            policy.contains("connect-src 'self' https://cloud.example.test;"),
            "{policy}"
        );
        assert!(!policy.contains("127.0.0.1:18080"), "the literal must not survive: {policy}");
    }

    /// A development build with a development file in the tree runs on that
    /// file, and says so.
    #[test]
    fn a_development_build_reads_the_tree_and_reports_it() {
        let text = config::PRODUCTION_TOML
            .replace("environment = \"production\"", "environment = \"development\"")
            .replace("port = 8765", "port = 8766");
        let dir = tree_with(&text, "dev");

        let startup = load(&BTreeMap::new(), Environment::Development, None, None, &dir);

        std::fs::remove_dir_all(&dir).ok();

        assert_eq!(startup.source, Source::DevelopmentTree);
        assert_eq!(startup.config.agent.port, 8766);
        assert!(startup.rejected.is_none());
        assert!(startup.summary().contains("开发树"), "{}", startup.summary());
    }

    /// A file that parses but is not usable falls back to the compiled-in
    /// default instead of aborting the launch — and the reason is kept, by key.
    #[test]
    fn a_rejected_file_falls_back_and_says_why_without_echoing_a_value() {
        let text = config::PRODUCTION_TOML
            .replace("environment = \"production\"", "environment = \"development\"")
            .replace("host = \"127.0.0.1\"", "host = \"0.0.0.0\"");
        let dir = tree_with(&text, "bad");

        let startup = load(&BTreeMap::new(), Environment::Development, None, None, &dir);

        std::fs::remove_dir_all(&dir).ok();

        assert_eq!(startup.config.agent.port, 8765, "the fallback must be the shipped config");
        assert_eq!(startup.source, Source::CompiledIn, "a rejected file is not the source");
        let reason = startup.rejected.as_ref().expect("the reason must be kept");
        assert!(reason.contains("agent.host"), "{reason}");
        assert!(!reason.contains("0.0.0.0"), "the value must not be echoed: {reason}");
        assert!(startup.summary().contains(reason), "{}", startup.summary());
    }

    /// Rule 1, end to end: in production, neither the locator nor the one
    /// development override has any effect, and `paths` is not what enforces
    /// that — both are ignored by two different modules and this is the test
    /// that sees them together.
    #[test]
    fn production_ignores_the_environment_end_to_end() {
        let text = config::PRODUCTION_TOML
            .replace("environment = \"production\"", "environment = \"development\"")
            .replace("port = 8765", "port = 9999");
        let dir = tree_with(&text, "prod");
        let env = env_of(&[
            (paths::CONFIG_ENV, dir.join("resources").join(paths::RESOURCE_NAME).to_str().unwrap()),
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
