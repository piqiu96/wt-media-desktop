//! Where the desktop configuration file comes from.
//!
//! Two layouts, one descending order each. A packaged app looks beside its own
//! resources first so an operator can replace the file without touching the
//! bundle; a development tree looks in the crate's `resources/`, because
//! `CARGO_MANIFEST_DIR` is the only location that is the same in every dev
//! checkout.
//!
//! Both orders end at [`crate::config::PRODUCTION_TOML`], which is compiled in.
//! That last step is the point of the module: a bundle whose resource file is
//! missing, truncated or corrupt still starts with a known-good config rather
//! than with no CSP and no Cloud address. A locator that could return "nothing"
//! would make that failure mode possible.
//!
//! Which layout applies is decided by the **build's** environment, not by the
//! file's — the file cannot be located until we know where to look for it. A
//! release binary is production unconditionally, so that is one fact, not two:
//! `Environment::Production` means both "look in the bundle" and "honour no
//! `WT_MEDIA_DESKTOP_*` variable", and passing them as separate arguments would
//! only create a way for them to disagree.
//!
//! `locate` is pure — the "does this path exist" question is a parameter — so
//! the descending order is testable without a filesystem or a real environment.
//! Reading the chosen file is [`file_text`], the only impure part.
//!
//! The caller is `bootstrap::resolve`, which supplies the two directories from
//! the running process and hands the text to `config::load_with`.

use crate::config::{Environment, PRODUCTION_TOML};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Points at the config file, overriding every other candidate.
///
/// Honoured in development only. `config::load_with` ignores the whole
/// `WT_MEDIA_DESKTOP_*` namespace once the effective environment is production,
/// but it cannot enforce that for *this* variable: the locator is what finds the
/// file that would have declared the environment, so the gate has to be here and
/// it has to key on the build's environment. A release bundle therefore cannot be
/// redirected at another file by whoever sets variables in its environment.
pub const CONFIG_ENV: &str = "WT_MEDIA_DESKTOP_CONFIG";

/// The file name looked for in the resource and manifest directories.
pub const RESOURCE_NAME: &str = "desktop.production.toml";

/// Which candidate supplied the text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Source {
    /// `WT_MEDIA_DESKTOP_CONFIG`.
    EnvOverride,
    /// The bundle's resource directory.
    BundledResource,
    /// Beside the executable.
    BesideExecutable,
    /// `<crate>/resources/` in a development checkout.
    DevelopmentTree,
    /// `config::PRODUCTION_TOML`, compiled into the binary.
    CompiledIn,
}

impl Source {
    /// The spelling a machine reads: which candidate supplied the config.
    ///
    /// Beside `bootstrap::Startup::summary`, which spells the same five in
    /// Chinese for the line an operator reads at launch. Two mappings rather than
    /// one because the two readers want different things — a person wants 「安装包的
    /// 资源目录」, a diagnostic bundle wants a value that survives being pasted into
    /// an issue — and the compiler holds them together from the other side: a new
    /// variant does not build until both matches have an arm for it.
    pub const fn code(self) -> &'static str {
        match self {
            Source::EnvOverride => "env_override",
            Source::BundledResource => "bundled_resource",
            Source::BesideExecutable => "beside_executable",
            Source::DevelopmentTree => "development_tree",
            Source::CompiledIn => "compiled_in",
        }
    }
}

/// The candidates in the order they are tried.
///
/// Ordering rationale, one step each: the operator's override beats everything
/// (in development only — see [`CONFIG_ENV`]); the resource directory is where a
/// bundle actually keeps it; beside the executable covers a bundle whose
/// resource directory was not resolved; the development tree is last because it
/// only exists in a checkout.
pub fn candidates(
    env: &BTreeMap<String, String>,
    environment: Environment,
    resource_dir: Option<&Path>,
    exe_dir: Option<&Path>,
    manifest_dir: &Path,
) -> Vec<(PathBuf, Source)> {
    let mut out = Vec::new();

    if environment == Environment::Development {
        if let Some(raw) = env.get(CONFIG_ENV) {
            let raw = raw.trim();
            if !raw.is_empty() {
                out.push((PathBuf::from(raw), Source::EnvOverride));
            }
        }
    }

    if environment == Environment::Production {
        if let Some(dir) = resource_dir {
            out.push((dir.join(RESOURCE_NAME), Source::BundledResource));
        }
        if let Some(dir) = exe_dir {
            out.push((dir.join(RESOURCE_NAME), Source::BesideExecutable));
        }
    } else {
        out.push((
            manifest_dir.join("resources").join(RESOURCE_NAME),
            Source::DevelopmentTree,
        ));
    }

    out
}

/// The first candidate that exists, or `None` if none do.
///
/// `exists` is injected so the order can be tested without a filesystem, and so
/// a test can make "nothing exists" true — the case that must fall through to
/// the compiled-in default.
pub fn locate(
    env: &BTreeMap<String, String>,
    environment: Environment,
    resource_dir: Option<&Path>,
    exe_dir: Option<&Path>,
    manifest_dir: &Path,
    exists: impl Fn(&Path) -> bool,
) -> Option<(PathBuf, Source)> {
    candidates(env, environment, resource_dir, exe_dir, manifest_dir)
        .into_iter()
        .find(|(path, _)| exists(path))
}

/// The config text to parse, and where it came from.
///
/// Never fails: a missing or unreadable file falls through to the next
/// candidate and finally to the compiled-in default, so the caller always has
/// something parseable to hand to `config::load_with`.
pub fn file_text(
    env: &BTreeMap<String, String>,
    environment: Environment,
    resource_dir: Option<&Path>,
    exe_dir: Option<&Path>,
    manifest_dir: &Path,
) -> (String, Source) {
    match locate(env, environment, resource_dir, exe_dir, manifest_dir, |p| {
        p.is_file()
    }) {
        Some((path, source)) => match std::fs::read_to_string(&path) {
            Ok(text) => (text, source),
            // The file existed when it was looked for and could not be read —
            // a permissions or encoding problem. Falling through rather than
            // panicking keeps a launch alive; `config::load_with` will reject
            // the default if it is somehow unusable, which it is not.
            Err(_) => (PRODUCTION_TOML.to_string(), Source::CompiledIn),
        },
        None => (PRODUCTION_TOML.to_string(), Source::CompiledIn),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn env_of(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn manifest() -> PathBuf {
        PathBuf::from("/crate")
    }

    /// Every source has its own code, and every code is a word a machine reads.
    ///
    /// The property is distinctness, not a spelling: a bundle that said two
    /// sources were the same file would send a reader to the wrong one, and that
    /// is exactly the failure a copy-paste slip in the `match` above produces.
    #[test]
    fn every_source_has_its_own_code() {
        let sources = [
            Source::EnvOverride,
            Source::BundledResource,
            Source::BesideExecutable,
            Source::DevelopmentTree,
            Source::CompiledIn,
        ];
        let codes: Vec<&str> = sources.iter().map(|source| source.code()).collect();
        let distinct: BTreeSet<&str> = codes.iter().copied().collect();
        assert_eq!(distinct.len(), sources.len(), "{codes:?}");
        for code in &codes {
            assert!(!code.is_empty(), "a code nobody can read is not a code");
            assert_eq!(
                *code,
                code.to_ascii_lowercase(),
                "the codes are the lowercase words the rest of the wire uses"
            );
        }
    }

    /// A production build honours no `WT_MEDIA_DESKTOP_*` variable, and the
    /// locator is the one that would otherwise slip past `config::load_with`:
    /// that module never sees this variable, so nothing downstream can enforce
    /// the rule. Without the gate here, a release bundle whose environment
    /// carries `WT_MEDIA_DESKTOP_CONFIG` reads a file chosen by whoever set it.
    ///
    /// The path in the variable is a real-looking one, and the assertion is on
    /// the *absence of the source*, not on the list being a particular length:
    /// a locator that is ignored must be ignored however many other candidates
    /// there are.
    #[test]
    fn a_production_build_ignores_the_locator() {
        let env = env_of(&[(CONFIG_ENV, "/operator/agent.toml")]);

        let got = candidates(
            &env,
            Environment::Production,
            Some(Path::new("/app/Resources")),
            Some(Path::new("/app/MacOS")),
            &manifest(),
        );

        assert!(
            !got.iter().any(|(_, source)| *source == Source::EnvOverride),
            "production must not honour the locator: {got:?}"
        );

        // The same variable *is* honoured in development — otherwise the test
        // above would also pass for a locator that never reads the variable at
        // all, which is a different (and broken) behaviour.
        let got = candidates(&env, Environment::Development, None, None, &manifest());
        assert_eq!(
            got.first(),
            Some(&(PathBuf::from("/operator/agent.toml"), Source::EnvOverride)),
            "development must honour the locator: {got:?}"
        );
    }

    /// The order a production build descends, and that it never consults the
    /// development tree.
    #[test]
    fn production_order_is_resource_then_exe() {
        let got = candidates(
            &BTreeMap::new(),
            Environment::Production,
            Some(Path::new("/app/Resources")),
            Some(Path::new("/app/MacOS")),
            &manifest(),
        );

        assert_eq!(
            got,
            vec![
                (
                    PathBuf::from("/app/Resources").join(RESOURCE_NAME),
                    Source::BundledResource
                ),
                (
                    PathBuf::from("/app/MacOS").join(RESOURCE_NAME),
                    Source::BesideExecutable
                ),
            ]
        );
    }

    /// A development tree descends through the override to its one file
    /// candidate, and no override means no first entry at all — not an
    /// empty-path candidate.
    #[test]
    fn development_order_is_override_then_manifest_resources_only() {
        let env = env_of(&[(CONFIG_ENV, "/operator/agent.toml")]);
        let got = candidates(
            &env,
            Environment::Development,
            Some(Path::new("/app/Resources")),
            Some(Path::new("/app/MacOS")),
            &manifest(),
        );
        assert_eq!(
            got,
            vec![
                (PathBuf::from("/operator/agent.toml"), Source::EnvOverride),
                (
                    manifest().join("resources").join(RESOURCE_NAME),
                    Source::DevelopmentTree
                ),
            ],
            "a development tree never consults the bundle's resource directory"
        );

        for blank in ["", "   "] {
            let env = env_of(&[(CONFIG_ENV, blank)]);
            let got = candidates(&env, Environment::Development, None, None, &manifest());
            assert_eq!(
                got.len(),
                1,
                "a blank override must not become a candidate: {blank:?}"
            );
        }
    }

    /// First match wins, and the search stops there.
    ///
    /// **Every** candidate exists in this case. With only one existing candidate
    /// the assertion would also hold for a locator that returns the *last*
    /// match, which is the opposite of the descending order this module exists
    /// to provide.
    #[test]
    fn locate_takes_the_first_candidate_that_exists() {
        // Production: the resource directory beats the executable's own.
        let found = locate(
            &BTreeMap::new(),
            Environment::Production,
            Some(Path::new("/app/Resources")),
            Some(Path::new("/app/MacOS")),
            &manifest(),
            |_| true,
        );
        assert_eq!(
            found,
            Some((
                Path::new("/app/Resources").join(RESOURCE_NAME),
                Source::BundledResource
            ))
        );

        // Development: the override beats the tree.
        let env = env_of(&[(CONFIG_ENV, "/operator/agent.toml")]);
        let found = locate(
            &env,
            Environment::Development,
            None,
            None,
            &manifest(),
            |_| true,
        );
        assert_eq!(
            found,
            Some((PathBuf::from("/operator/agent.toml"), Source::EnvOverride)),
            "with every candidate present, the override must still win"
        );
    }

    /// Nothing on disk is a supported outcome, and it is the one the compiled-in
    /// default exists for.
    #[test]
    fn no_candidate_existing_falls_through_to_the_compiled_default() {
        assert_eq!(
            locate(
                &BTreeMap::new(),
                Environment::Production,
                None,
                None,
                &manifest(),
                |_| false
            ),
            None
        );

        let (text, source) = file_text(
            &BTreeMap::new(),
            Environment::Production,
            None,
            None,
            &manifest(),
        );
        assert_eq!(source, Source::CompiledIn);
        assert_eq!(
            text, PRODUCTION_TOML,
            "the fallback must be the shipped config"
        );
    }

    /// A file that exists but cannot be read must fall back, not panic.
    ///
    /// This is the one branch of `file_text` the pure tests cannot reach, and it
    /// was untested until a mutation that turned the fallback into a `panic!`
    /// survived. Non-UTF-8 bytes are how the case is made reachable here:
    /// `read_to_string` rejects them, and unlike a permissions trick it behaves
    /// the same for a root user and in CI.
    #[test]
    fn an_existing_file_that_cannot_be_read_falls_back_instead_of_panicking() {
        let dir = std::env::temp_dir().join(format!("wt-media-paths-test-{}", std::process::id()));
        let resources = dir.join("resources");
        std::fs::create_dir_all(&resources).expect("temp dir");
        let path = resources.join(RESOURCE_NAME);
        std::fs::write(&path, [0xff, 0xfe, 0x00]).expect("write non-UTF-8 config");

        let (text, source) =
            file_text(&BTreeMap::new(), Environment::Development, None, None, &dir);

        std::fs::remove_dir_all(&dir).ok();

        assert_eq!(
            source,
            Source::CompiledIn,
            "an unreadable file must not be reported as used"
        );
        assert_eq!(text, PRODUCTION_TOML);
    }

    /// The default parses — otherwise the fallback would trade one broken start
    /// for another. This is the only assertion here that reaches `config`.
    #[test]
    fn the_compiled_default_is_a_usable_config() {
        let (text, source) = file_text(
            &BTreeMap::new(),
            Environment::Production,
            None,
            None,
            &manifest(),
        );
        assert_eq!(source, Source::CompiledIn);
        let parsed = crate::config::load_with(&BTreeMap::new(), &text, Environment::Production);
        assert!(
            parsed.is_ok(),
            "the fallback must parse and validate: {parsed:?}"
        );
    }
}
