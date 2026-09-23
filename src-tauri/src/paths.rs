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
//! `locate` is pure — the "does this path exist" question is a parameter — so
//! the descending order is testable without a filesystem or a real environment.
//! Reading the chosen file is [`file_text`], the only impure part.
//!
//! Not wired yet: T-07 calls this from `main` and feeds the text to
//! `config::load_with`. Until then this module has no consumer.

use crate::config::PRODUCTION_TOML;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Points at the config file, overriding every other candidate. Honoured in
/// development only: `config::load_with` ignores the whole `WT_MEDIA_DESKTOP_*`
/// namespace once the effective environment is production, and that rule is
/// enforced there rather than here so there is one place to audit.
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
    /// Whether this source is a file on disk (as opposed to compiled in).
    pub fn is_file(self) -> bool {
        self != Source::CompiledIn
    }
}

/// The candidates in the order they are tried.
///
/// Ordering rationale, one step each: the operator's override beats everything;
/// the resource directory is where a bundle actually keeps it; beside the
/// executable covers a bundle whose resource directory was not resolved; the
/// development tree is last because it only exists in a checkout.
pub fn candidates(
    env: &BTreeMap<String, String>,
    bundled: bool,
    resource_dir: Option<&Path>,
    exe_dir: Option<&Path>,
    manifest_dir: &Path,
) -> Vec<(PathBuf, Source)> {
    let mut out = Vec::new();

    if let Some(raw) = env.get(CONFIG_ENV) {
        let raw = raw.trim();
        if !raw.is_empty() {
            out.push((PathBuf::from(raw), Source::EnvOverride));
        }
    }

    if bundled {
        if let Some(dir) = resource_dir {
            out.push((dir.join(RESOURCE_NAME), Source::BundledResource));
        }
        if let Some(dir) = exe_dir {
            out.push((dir.join(RESOURCE_NAME), Source::BesideExecutable));
        }
    } else {
        out.push((manifest_dir.join("resources").join(RESOURCE_NAME), Source::DevelopmentTree));
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
    bundled: bool,
    resource_dir: Option<&Path>,
    exe_dir: Option<&Path>,
    manifest_dir: &Path,
    exists: impl Fn(&Path) -> bool,
) -> Option<(PathBuf, Source)> {
    candidates(env, bundled, resource_dir, exe_dir, manifest_dir)
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
    bundled: bool,
    resource_dir: Option<&Path>,
    exe_dir: Option<&Path>,
    manifest_dir: &Path,
) -> (String, Source) {
    match locate(env, bundled, resource_dir, exe_dir, manifest_dir, |p| p.is_file()) {
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

    fn env_of(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn manifest() -> PathBuf {
        PathBuf::from("/crate")
    }

    /// The order a packaged app descends, and that it never consults the
    /// development tree.
    #[test]
    fn bundled_order_is_override_resource_exe() {
        let env = env_of(&[(CONFIG_ENV, "/operator/agent.toml")]);
        let got = candidates(
            &env,
            true,
            Some(Path::new("/app/Resources")),
            Some(Path::new("/app/MacOS")),
            &manifest(),
        );

        assert_eq!(
            got,
            vec![
                (PathBuf::from("/operator/agent.toml"), Source::EnvOverride),
                (PathBuf::from("/app/Resources").join(RESOURCE_NAME), Source::BundledResource),
                (PathBuf::from("/app/MacOS").join(RESOURCE_NAME), Source::BesideExecutable),
            ]
        );
    }

    /// A development tree has exactly one file candidate, and no override means
    /// no first entry at all — not an empty-path candidate.
    #[test]
    fn development_order_is_manifest_resources_only() {
        let got = candidates(&BTreeMap::new(), false, Some(Path::new("/app/Resources")), None, &manifest());
        assert_eq!(
            got,
            vec![(manifest().join("resources").join(RESOURCE_NAME), Source::DevelopmentTree)]
        );

        for blank in ["", "   "] {
            let env = env_of(&[(CONFIG_ENV, blank)]);
            let got = candidates(&env, false, None, None, &manifest());
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
        let env = env_of(&[(CONFIG_ENV, "/operator/agent.toml")]);
        let found = locate(
            &env,
            true,
            Some(Path::new("/app/Resources")),
            Some(Path::new("/app/MacOS")),
            &manifest(),
            |_| true,
        );
        assert_eq!(
            found,
            Some((PathBuf::from("/operator/agent.toml"), Source::EnvOverride)),
            "with all candidates present, the override must still win"
        );

        // And with the override absent, the resource directory beats the
        // executable's own directory.
        let found = locate(&BTreeMap::new(), true, Some(Path::new("/app/Resources")),
                           Some(Path::new("/app/MacOS")), &manifest(), |_| true);
        assert_eq!(
            found,
            Some((
                Path::new("/app/Resources").join(RESOURCE_NAME),
                Source::BundledResource
            ))
        );
    }

    /// Nothing on disk is a supported outcome, and it is the one the compiled-in
    /// default exists for.
    #[test]
    fn no_candidate_existing_falls_through_to_the_compiled_default() {
        assert_eq!(
            locate(&BTreeMap::new(), true, None, None, &manifest(), |_| false),
            None
        );

        let (text, source) = file_text(&BTreeMap::new(), true, None, None, &manifest());
        assert_eq!(source, Source::CompiledIn);
        assert_eq!(text, PRODUCTION_TOML, "the fallback must be the shipped config");
        assert!(!source.is_file());
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

        let (text, source) = file_text(&BTreeMap::new(), false, None, None, &dir);

        std::fs::remove_dir_all(&dir).ok();

        assert_eq!(source, Source::CompiledIn, "an unreadable file must not be reported as used");
        assert_eq!(text, PRODUCTION_TOML);
    }

    /// The default parses — otherwise the fallback would trade one broken start
    /// for another. This is the only assertion here that reaches `config`.
    #[test]
    fn the_compiled_default_is_a_usable_config() {
        let (text, source) = file_text(&BTreeMap::new(), false, None, None, &manifest());
        assert_eq!(source, Source::CompiledIn);
        let parsed = crate::config::load_with(
            &BTreeMap::new(),
            &text,
            crate::config::Environment::Production,
        );
        assert!(parsed.is_ok(), "the fallback must parse and validate: {parsed:?}");
    }

    /// Every source except the compiled-in one names a file on disk, so a caller
    /// can tell "we read a file" from "we fell back" without matching on the
    /// enum. Getting this backwards would make a fallback look like a file.
    #[test]
    fn only_the_compiled_default_is_not_a_file() {
        for (source, is_file) in [
            (Source::EnvOverride, true),
            (Source::BundledResource, true),
            (Source::BesideExecutable, true),
            (Source::DevelopmentTree, true),
            (Source::CompiledIn, false),
        ] {
            assert_eq!(source.is_file(), is_file, "{source:?}");
        }
    }
}
