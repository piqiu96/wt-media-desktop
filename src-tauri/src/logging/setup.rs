//! Installing the subscriber: the one place that touches the process.
//!
//! Everything else in this module tree is decisions — where the directory is,
//! what is recorded, how a line reads, when a file rolls. Those are all pure
//! enough to test on their own. This is the part that cannot be: a global
//! subscriber is process-wide and installable exactly once, so anything left
//! inside `install` is only reachable by launching the program. That is the
//! whole reason `plan` exists next to it — the decisions are made where a test
//! can ask about them, and `install` is left with nothing to decide.
//!
//! **Which environment.** The directory and the default level follow the
//! **build**, not the environment the config file declares. The two are not the
//! same value: the shipped file says `production` and `load_with` takes the
//! stricter of it and the build (`config.rs`), so a debug build running the
//! shipped file has an effective environment of *production*. Keying the log
//! destination off that value would put a developer's records in
//! `~/Library/Logs/WTMedia/Desktop` — and would make the development layout in
//! `paths` unreachable on every ordinary launch. The CHG registers the
//! consequence: a process can now have two "environments", so the launch
//! summary names both.
//!
//! **When.** Before anything else can want to say something. The launch summary
//! is the first record in the file, and a subscriber installed one line later
//! would leave everything said before it nowhere at all.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tracing::level_filters::LevelFilter;

use crate::config::{DesktopConfig, Environment, LOG_LEVEL_AUTO};
use crate::logging::rolling::Limits;
use crate::logging::targets::Levels;
use crate::logging::{backend, paths};

/// What the launch did about logging, and what a reader has to know about it.
///
/// `installed: false` is the one case the caller must not ignore: a subscriber
/// was already installed, so this process's records go nowhere and the summary
/// has to reach stderr some other way.
pub struct Installed {
    /// Where the file sink writes. `None` is a launch that only has stderr.
    pub directory: Option<PathBuf>,
    /// Why there is no directory, if there is none.
    pub problem: Option<String>,
    /// The level in force for targets that are not configured separately.
    pub level: LevelFilter,
    /// The token that level came from, so a reader can tell `auto` from `info`.
    pub configured_level: String,
    /// The environment the config file settled on (the stricter of the file's
    /// declaration and the build).
    pub config_environment: Environment,
    /// The environment the build is; what the directory and level follow.
    pub build_environment: Environment,
    /// Whether this call is the one that installed the process-wide subscriber.
    pub installed: bool,
}

impl Installed {
    /// The logging half of the launch summary, in one line.
    ///
    /// Names the four facts a diagnosis starts from — where the records go,
    /// what level they run at, what the file said, and which of the two
    /// environments is which. The last one is spelled out only when the two
    /// disagree, which is the case a reader will not expect.
    pub fn summary(&self) -> String {
        let destination = match (&self.directory, &self.problem) {
            (Some(directory), _) => format!("日志目录 {}", directory.display()),
            (None, Some(problem)) => format!("日志目录不可用（{problem}），本次只写 stderr"),
            // Unreachable: no directory always carries a reason. Written out
            // rather than `unwrap`ed so a future edit that drops the reason
            // shows up as a missing clause instead of a panic before any window.
            (None, None) => "日志目录不可用，本次只写 stderr".to_string(),
        };
        let environment = if self.config_environment == self.build_environment {
            format!("环境 {}", name_of(self.build_environment))
        } else {
            format!(
                "环境 {}（构建 {}）",
                name_of(self.config_environment),
                name_of(self.build_environment)
            )
        };
        format!(
            "{}；级别 {}（配置 {}）；{}",
            destination,
            level_name(self.level),
            self.configured_level,
            environment
        )
    }
}

/// What a launch decided about logging, before anything process-wide happens.
///
/// Every decision is here and not in `install` for one reason: the subscriber
/// can be installed once per process, so whatever stays inside `install` can
/// only be reached by launching the program — and a rule that can only be
/// checked by eye is a rule that drifts. The split is not decorative: T-15's
/// first mutation made `install` resolve the directory from
/// `config.environment` (the wrong environment, see this module's header) and
/// the whole suite stayed green. With the resolution here, that mistake is a
/// red test.
pub struct Plan {
    /// The levels the subscriber will run under.
    pub levels: Levels,
    /// The budgets the writer will run under.
    pub limits: Limits,
    /// The directory, or the reason there is none. An unusable directory is a
    /// value rather than an error because the launch continues either way.
    pub directory: Result<PathBuf, String>,
    /// The token `levels` came from, so a reader can tell `auto` from `info`.
    pub configured_level: String,
    /// The environment the config file settled on.
    pub config_environment: Environment,
    /// The environment the build is; what `levels` and `directory` follow.
    pub build_environment: Environment,
}

/// Decide where this launch logs and at what level.
///
/// Pure apart from `paths::prepare`'s create-and-probe, and every input is a
/// parameter — including the build environment, which is the fact the whole
/// module turns on.
pub fn plan(
    config: &DesktopConfig,
    build_environment: Environment,
    home: Option<&Path>,
    manifest_dir: &Path,
) -> Plan {
    Plan {
        levels: levels(&config.logging.level, build_environment),
        limits: limits_of(config),
        directory: resolve_directory(build_environment, home, manifest_dir),
        configured_level: config.logging.level.clone(),
        config_environment: config.environment,
        build_environment,
    }
}

/// Carry out a `Plan`: assemble the subscriber and install it.
///
/// Never fails and never panics: every input it cannot use becomes a `None`
/// field plus a note on stderr. The ruling is that a logging problem may not
/// stop a launch, and the way to keep that promise at the one place that
/// touches the process is for this function to have no `Err` arm and no
/// `unwrap`.
///
/// Takes no configuration, by construction: the only thing left to decide here
/// is what a `Plan` already settled.
pub fn install(plan: Plan, secrets: Vec<String>) -> Installed {
    let Plan {
        levels,
        limits,
        directory,
        configured_level,
        config_environment,
        build_environment,
    } = plan;
    let (directory, problem) = match directory {
        Ok(directory) => (Some(directory), None),
        Err(problem) => {
            backend::note(&problem);
            (None, Some(problem))
        }
    };

    let subscriber = backend::assemble(backend::Options {
        levels: levels.clone(),
        secrets,
        directory: directory.clone(),
        limits,
        clock: Arc::new(backend::SystemClock),
    });
    let installed = match tracing::subscriber::set_global_default(subscriber) {
        Ok(()) => true,
        // Nothing in this program installs a subscriber before this point, so
        // this arm is here for the day something does: the records of whichever
        // subscriber won are not ours to replace mid-launch, and a panic here
        // would be a launch that cannot happen at all.
        Err(error) => {
            backend::note(&format!("the log subscriber was not installed: {error}"));
            false
        }
    };

    Installed {
        directory,
        problem,
        level: levels.default,
        configured_level,
        config_environment,
        build_environment,
        installed,
    }
}

/// The level set a configuration token names.
///
/// `auto` is the environment's shipped levels (production INFO / development
/// DEBUG, ruling 四) — a token rather than an absent key, so the file states the
/// decision and `config::validate` can check it. Any other accepted token
/// replaces the default and leaves the per-target rules alone, which is what
/// keeps `agent.supervisor` from being silenced by a quiet `level` (see
/// `targets::effective_level`).
pub fn levels(configured: &str, build_environment: Environment) -> Levels {
    if configured == LOG_LEVEL_AUTO {
        return Levels::shipped(build_environment);
    }
    match filter_of(configured) {
        Some(default) => Levels {
            default,
            per_target: BTreeMap::new(),
        },
        // Unreachable with a loaded config: `validate` rejects every other
        // token, so a file that got this far already named an accepted level.
        // The shipped levels are the fallback rather than a panic — and the
        // summary prints the level that is in force, so the substitution is
        // visible in the record it affects.
        None => Levels::shipped(build_environment),
    }
}

/// The filter one accepted token names.
///
/// A table rather than a parse: `LevelFilter::from_str` also accepts `"1"`,
/// `"03"` and upper case, so a spelled-out match is what keeps the accepted
/// spellings — and *only* those — coming from `config::LOG_LEVELS`. `None` for
/// anything else, including `auto`, which is a level *set* and not a level.
pub fn filter_of(token: &str) -> Option<LevelFilter> {
    Some(match token {
        "off" => LevelFilter::OFF,
        "error" => LevelFilter::ERROR,
        "warn" => LevelFilter::WARN,
        "info" => LevelFilter::INFO,
        "debug" => LevelFilter::DEBUG,
        "trace" => LevelFilter::TRACE,
        _ => return None,
    })
}

/// The two numbers the writer runs under, from `[logging]`.
pub fn limits_of(config: &DesktopConfig) -> Limits {
    Limits {
        retention_days: config.logging.retention_days,
        max_record_bytes: config.logging.max_record_bytes,
    }
}

/// The log directory for this build, created and proved writable.
///
/// The environment is the **build's**; see this module's header for why it is
/// not `config.environment`. `home` is `None` when the process has no `HOME`,
/// which only matters for the installed layout — the development layout is
/// built from the manifest directory and never reads it.
fn resolve_directory(
    build_environment: Environment,
    home: Option<&Path>,
    manifest_dir: &Path,
) -> Result<PathBuf, String> {
    let prepared = match (build_environment, home) {
        // The empty path is a placeholder `paths::directory` ignores in this arm
        // by construction (`paths::tests::
        // each_layout_reads_one_input_and_ignores_the_other` pins that it does),
        // so a missing `HOME` cannot take the development layout away.
        (Environment::Development, _) => {
            paths::prepare(Path::new(""), build_environment, manifest_dir)
        }
        (Environment::Production, Some(home)) => {
            paths::prepare(home, build_environment, manifest_dir)
        }
        (Environment::Production, None) => {
            return Err("HOME is not set, so the installed layout has no directory".to_string())
        }
    };
    prepared.map_err(|error| error.to_string())
}

/// The environment's name as the summary spells it.
fn name_of(environment: Environment) -> &'static str {
    match environment {
        Environment::Development => "development",
        Environment::Production => "production",
    }
}

/// The level's name as a record spells it.
///
/// `LevelFilter`'s own `Display` is lower case, and the only other place a level
/// is written in this file is the `[INFO]` of a record — a summary reading
/// `级别 info` would spell it differently from the lines underneath it. `OFF` is
/// the one filter that is not a level, so it is named here; it is also the only
/// one with no records to agree with.
fn level_name(filter: LevelFilter) -> &'static str {
    filter.into_level().map_or("OFF", |level| level.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{load_with, LOG_LEVELS, PRODUCTION_TOML};

    fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "wt-media-logging-setup-{}-{}-{}",
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

    fn home() -> PathBuf {
        PathBuf::from("/home/operator")
    }

    fn shipped() -> DesktopConfig {
        load_with(&BTreeMap::new(), PRODUCTION_TOML, Environment::Production)
            .expect("the shipped resource must load")
    }

    /// The accepted tokens and the filter each names, written out by hand.
    ///
    /// Test-only duplication on purpose, and the second time this CHG needs it:
    /// every other answer in this file is derived from `config::LOG_LEVELS` or
    /// from `filter_of`, so a wrong *value* in the mapping moves the
    /// expectation along with itself — which is exactly how T-14's `LOG_LEVELS`
    /// mutation survived. This table is the one place a level has to be edited
    /// by hand, and `every_accepted_token_maps_to_this_filter` ties its token
    /// list back to `config::LOG_LEVELS` so the two cannot drift apart.
    const PINNED_LEVELS: [(&str, LevelFilter); 6] = [
        ("off", LevelFilter::OFF),
        ("error", LevelFilter::ERROR),
        ("warn", LevelFilter::WARN),
        ("info", LevelFilter::INFO),
        ("debug", LevelFilter::DEBUG),
        ("trace", LevelFilter::TRACE),
    ];

    /// The pinned filter for a token, so a token the table has never heard of
    /// fails loudly instead of quietly comparing against nothing.
    fn pinned(token: &str) -> LevelFilter {
        PINNED_LEVELS
            .iter()
            .find(|(name, _)| *name == token)
            .unwrap_or_else(|| panic!("config accepts {token:?}, which this table does not pin"))
            .1
    }

    /// A summary-bearing value with everything set; each case changes one field.
    fn installed() -> Installed {
        Installed {
            directory: Some(PathBuf::from("/tmp/logs")),
            problem: None,
            level: LevelFilter::INFO,
            configured_level: LOG_LEVEL_AUTO.to_string(),
            config_environment: Environment::Production,
            build_environment: Environment::Production,
            installed: true,
        }
    }

    /// The destination and the level follow the **build**, even though the
    /// config file makes the two environments disagree on every ordinary launch.
    ///
    /// The premise is half the test: the shipped file declares production and
    /// `load_with` takes the stricter of file and build, so a debug build's
    /// effective environment is production. If the plan were resolved from that
    /// value, the development layout would be unreachable and a developer's
    /// records would land in the installed location — which is why the build is
    /// what gets passed in, and why this test asks `plan` rather than the
    /// helpers it delegates to.
    #[test]
    fn the_plan_follows_the_build_not_the_effective_environment() {
        let mut config = shipped();
        assert_eq!(
            config.environment,
            Environment::Production,
            "the premise: the shipped file under a debug build is effectively production"
        );

        let manifest = scratch("plan");
        std::fs::create_dir_all(&manifest).expect("scratch");
        // A writable stand-in for the home: `paths::prepare` creates the
        // directory it returns, so the installed direction cannot be asked with
        // a home that does not exist — and must never be asked with the real one.
        let installed_home = scratch("plan-home");
        std::fs::create_dir_all(&installed_home).expect("scratch");

        let development = plan(&config, Environment::Development, Some(&home()), &manifest);
        let directory = development
            .directory
            .as_ref()
            .expect("the development layout must be usable in a writable tree");
        assert_eq!(directory, &manifest.join(".local").join("logs"));
        assert!(
            !directory.starts_with(home()),
            "the development layout must not read the home: {directory:?}"
        );
        assert_eq!(
            development.levels.default,
            LevelFilter::DEBUG,
            "the development build's `auto` is DEBUG (ruling 四)"
        );
        assert!(development.directory.as_ref().expect("usable").exists());

        // The other direction, so this is a rule rather than a constant.
        let production = plan(
            &config,
            Environment::Production,
            Some(&installed_home),
            &manifest,
        );
        let directory = production.directory.as_ref().expect("a writable home");
        assert_eq!(
            directory,
            &installed_home
                .join("Library")
                .join("Logs")
                .join("WTMedia")
                .join("Desktop")
        );
        assert!(
            directory.exists(),
            "`prepare` proves the directory it returns, so it must be there"
        );
        assert_eq!(
            production.levels.default,
            LevelFilter::INFO,
            "the production build's `auto` is INFO"
        );

        assert_eq!(
            plan(&config, Environment::Production, None, &manifest).directory,
            Err("HOME is not set, so the installed layout has no directory".to_string())
        );

        // The numbers travel with the plan rather than being re-decided at
        // installation time.
        config.logging.max_record_bytes = 4096;
        assert_eq!(
            plan(&config, Environment::Development, None, &manifest)
                .limits
                .max_record_bytes,
            4096
        );

        // And so does the token the file declared, rather than the one this
        // code would have picked: the summary names what the file said, so a
        // hard-wired `auto` there would describe a launch that did not happen.
        config.logging.level = "trace".to_string();
        let explicit = plan(&config, Environment::Production, None, &manifest);
        assert_eq!(explicit.configured_level, "trace");
        assert_eq!(explicit.levels.default, LevelFilter::TRACE);

        std::fs::remove_dir_all(&manifest).ok();
        std::fs::remove_dir_all(&installed_home).ok();
    }

    /// An unusable directory is a reason, not an error: the caller has no policy
    /// to invent, and the launch continues on stderr.
    #[test]
    fn an_unusable_directory_is_a_reason_and_not_a_failure() {
        let manifest = scratch("unusable");
        std::fs::create_dir_all(&manifest).expect("scratch");

        // A file where the tree would go — the same shape the occupied-directory
        // arm of the real launch uses.
        std::fs::write(manifest.join(".local"), b"occupied").expect("the blocker");
        let problem = plan(
            &shipped(),
            Environment::Development,
            Some(&home()),
            &manifest,
        )
        .directory
        .expect_err("a file where the directory would go must be reported");
        assert!(problem.contains(".local"), "{problem}");
        std::fs::remove_dir_all(&manifest).ok();
    }

    /// The mapping itself, against hand-written pairs.
    ///
    /// A round trip through `filter_of` alone cannot see a wrong *value* — both
    /// sides move together — so the expected filter is a literal here, and the
    /// token list is tied back to `config::LOG_LEVELS` by `join` (a length
    /// change is a red test rather than a compile error, and T-14 measured why:
    /// a mutation that does not compile proves nothing).
    #[test]
    fn every_accepted_token_maps_to_this_filter() {
        let tokens: Vec<&str> = PINNED_LEVELS.iter().map(|(token, _)| *token).collect();
        assert_eq!(
            tokens.join(" "),
            LOG_LEVELS.join(" "),
            "the pinned table and the config vocabulary must be the same list"
        );

        for (token, expected) in PINNED_LEVELS {
            assert_eq!(filter_of(token), Some(expected), "{token}");
            assert_eq!(
                levels(token, Environment::Production).default,
                expected,
                "{token} must not be re-interpreted on the way in"
            );
        }
    }

    /// Every token `config` accepts is mapped here, `auto` is a set and not a
    /// level, and no other spelling gets in.
    ///
    /// `LevelFilter::from_str` would have taken `"INFO"`, `"1"` and `"03"` too —
    /// spellings the config layer rejects, which a mapping that parsed instead
    /// of matching would have quietly accepted.
    #[test]
    fn every_accepted_token_maps_and_auto_is_the_sentinel() {
        for token in LOG_LEVELS {
            let direct = pinned(token);
            let configured = levels(token, Environment::Production);
            assert_eq!(configured.default, direct);

            // Asserted where it takes effect: `for_target` answers with the
            // configured level, and the floor is applied when the filter is
            // built. A token of `off` still has to leave the supervision target
            // audible — and, for the same token, everything else silent, or a
            // floor that enabled all three targets would pass this too.
            let filter = configured.filter();
            assert!(
                filter.would_enable("agent.supervisor", &tracing::Level::INFO),
                "{token} must not silence the supervision target"
            );
            let others = ["desktop.startup", "webview"];
            for target in others {
                assert_eq!(
                    filter.would_enable(target, &tracing::Level::INFO),
                    direct >= LevelFilter::INFO,
                    "{token}: {target} must follow the configured level"
                );
            }
        }

        assert_eq!(filter_of(LOG_LEVEL_AUTO), None);
        for spelling in ["INFO", "1", "03", "off ", "auto"] {
            assert_eq!(
                filter_of(spelling),
                None,
                "{spelling:?} must not be a token"
            );
        }

        assert_eq!(
            levels(LOG_LEVEL_AUTO, Environment::Production).default,
            LevelFilter::INFO
        );
        assert_eq!(
            levels(LOG_LEVEL_AUTO, Environment::Development).default,
            LevelFilter::DEBUG
        );
    }

    /// The numbers are the ones the file declares, not re-defaulted here.
    #[test]
    fn the_limits_are_the_configured_ones() {
        let mut config = shipped();
        assert_eq!(limits_of(&config), Limits::SHIPPED);

        config.logging.retention_days = 2;
        config.logging.max_record_bytes = 8192;
        assert_eq!(
            limits_of(&config),
            Limits {
                retention_days: 2,
                max_record_bytes: 8192
            }
        );
    }

    /// The summary names all four facts, and says which environment is which
    /// only when they disagree.
    #[test]
    fn the_summary_names_the_destination_the_level_and_both_environments() {
        let summary = installed().summary();
        assert!(summary.contains("/tmp/logs"), "{summary}");
        assert!(summary.contains("INFO"), "{summary}");
        assert!(summary.contains("auto"), "{summary}");
        assert!(summary.contains("环境 production"), "{summary}");
        assert!(
            !summary.contains("构建"),
            "one environment needs no gloss: {summary}"
        );

        let disagreeing = Installed {
            build_environment: Environment::Development,
            ..installed()
        }
        .summary();
        assert!(
            disagreeing.contains("环境 production（构建 development）"),
            "{disagreeing}"
        );

        let no_directory = Installed {
            directory: None,
            problem: Some("log directory /x is unusable: permission denied".to_string()),
            ..installed()
        }
        .summary();
        assert!(no_directory.contains("stderr"), "{no_directory}");
        assert!(no_directory.contains("permission denied"), "{no_directory}");
    }
}
