//! Which records are Desktop's, and at what level they run.
//!
//! Desktop logs what **Desktop** does. Everything else has a home already: the
//! Agent's own three files are the Agent's business, and copying them here
//! would pay twice for the same record and give a reader two places to look
//! (the user's ruling 七).
//!
//! That distinction has to be enforced by more than a convention, because
//! `tracing` targets come from everywhere: `h2`, `hyper-util` and `softbuffer`
//! are already in the tree and already emit into whatever subscriber exists. A
//! subscriber that accepts what it is given fills the file with connection
//! frames and window events at exactly the moment the file is needed. So the
//! filter starts **off** and each of Desktop's own targets is turned back on by
//! name -- the default is silence, and a target added here is a decision.
//!
//! The vocabulary is fixed (Q-07): it is the key a log viewer filters on, so a
//! target that is not in [`OWNED_TARGETS`] has nowhere to be logged from.

use std::collections::BTreeMap;

use tracing::level_filters::LevelFilter;
use tracing_subscriber::filter::Targets;

use crate::config::Environment;

/// Every target Desktop logs under.
///
/// `agent.supervisor` -- starting and stopping the Agent, its health, its
/// sidecar exiting. `desktop.startup` -- the launch itself and the config it
/// settled on. `webview` -- what the shell reports back from the front end.
pub const OWNED_TARGETS: [&str; 3] = ["agent.supervisor", "desktop.startup", "webview"];

/// The one target that reports on another process.
///
/// A supervision event matters most when something is already wrong, and the
/// level that would hide it is the level nobody will be running to change: if
/// the Agent dies at ERROR-with-no-errors-recorded, the file that was supposed
/// to explain the death is empty. So this target is never held stricter than
/// INFO. Looser is fine -- DEBUG is not capped back down.
pub const SUPERVISION_TARGET: &str = "agent.supervisor";

/// The level a target actually runs at, whatever the config asked for.
pub fn effective_level(target: &str, configured: LevelFilter) -> LevelFilter {
    if target == SUPERVISION_TARGET {
        // `OFF < ERROR < WARN < INFO < DEBUG < TRACE`, so the looser level is
        // the greater one.
        configured.max(LevelFilter::INFO)
    } else {
        configured
    }
}

/// What to record, per environment and per target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Levels {
    /// The level for every target not named in `per_target`.
    pub default: LevelFilter,
    /// The exceptions, by target.
    pub per_target: BTreeMap<String, LevelFilter>,
}

impl Levels {
    /// The shipped levels: production INFO, development DEBUG (the ruling).
    ///
    /// Production is the quiet one because INFO there may not carry request and
    /// response detail; development is the loud one because that is where the
    /// detail is the point.
    pub fn shipped(environment: Environment) -> Levels {
        Levels {
            default: match environment {
                Environment::Production => LevelFilter::INFO,
                Environment::Development => LevelFilter::DEBUG,
            },
            per_target: BTreeMap::new(),
        }
    }

    /// The level configured for one target.
    pub fn for_target(&self, target: &str) -> LevelFilter {
        self.per_target.get(target).copied().unwrap_or(self.default)
    }

    /// The filter a sink is built with.
    ///
    /// `with_default(OFF)` first: an unowned target is not quieter, it is
    /// absent. Each owned target is then turned on at its effective level, so
    /// no target of ours can be lost to the default by omission.
    pub fn filter(&self) -> Targets {
        let mut targets = Targets::new().with_default(LevelFilter::OFF);
        for target in OWNED_TARGETS {
            targets = targets.with_target(target, effective_level(target, self.for_target(target)));
        }
        targets
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing::Level;

    #[test]
    fn the_vocabulary_is_the_one_the_ruling_names() {
        // A stable routing fact (Q-07): a log viewer filters on these strings,
        // so they are asserted rather than left to whatever the code happens to
        // use today.
        assert_eq!(
            OWNED_TARGETS,
            ["agent.supervisor", "desktop.startup", "webview"]
        );
        assert!(OWNED_TARGETS.contains(&SUPERVISION_TARGET));
        let unique: std::collections::BTreeSet<&str> = OWNED_TARGETS.iter().copied().collect();
        assert_eq!(
            unique.len(),
            OWNED_TARGETS.len(),
            "a target is listed twice"
        );
    }

    #[test]
    fn supervision_is_never_held_stricter_than_info() {
        // Every level that would hide an INFO record is raised to INFO...
        for stricter in [LevelFilter::OFF, LevelFilter::ERROR, LevelFilter::WARN] {
            assert_eq!(
                effective_level(SUPERVISION_TARGET, stricter),
                LevelFilter::INFO
            );
        }
        // ...and a looser one is not capped back down: DEBUG is not a level
        // anything needs protecting from.
        for looser in [LevelFilter::INFO, LevelFilter::DEBUG, LevelFilter::TRACE] {
            assert_eq!(effective_level(SUPERVISION_TARGET, looser), looser);
        }
    }

    #[test]
    fn the_floor_belongs_to_the_supervision_target_alone() {
        for target in ["webview", "desktop.startup"] {
            for level in [LevelFilter::OFF, LevelFilter::ERROR, LevelFilter::WARN] {
                assert_eq!(effective_level(target, level), level, "{target}");
            }
        }
    }

    #[test]
    fn the_shipped_levels_are_production_info_and_development_debug() {
        assert_eq!(
            Levels::shipped(Environment::Production).default,
            LevelFilter::INFO
        );
        assert_eq!(
            Levels::shipped(Environment::Development).default,
            LevelFilter::DEBUG
        );
    }

    #[test]
    fn a_target_without_an_override_takes_the_default() {
        let mut levels = Levels::shipped(Environment::Production);
        levels
            .per_target
            .insert("webview".to_owned(), LevelFilter::DEBUG);
        assert_eq!(levels.for_target("webview"), LevelFilter::DEBUG);
        assert_eq!(levels.for_target("desktop.startup"), LevelFilter::INFO);
    }

    #[test]
    fn the_filter_is_off_for_every_target_desktop_does_not_own() {
        // The one that matters: `h2`, `hyper-util` and `softbuffer` are already
        // emitting into whatever subscriber exists, and the level here is the
        // loudest there is, so nothing but the allowlist can keep them out.
        let filter = Levels::shipped(Environment::Development).filter();
        assert!(!filter.would_enable("h2::codec", &Level::TRACE));
        assert!(!filter.would_enable("hyper_util::client", &Level::ERROR));
        assert!(!filter.would_enable("softbuffer", &Level::ERROR));
        // The control arm: the same filter is not simply off.
        assert!(filter.would_enable("desktop.startup", &Level::DEBUG));
    }

    #[test]
    fn every_owned_target_is_selectable_at_the_level_it_runs_at() {
        // The enumerated assertion the plan asks for: the filter's default is
        // OFF, so a target missing from the allowlist is silently dropped.
        // Iterating the list means a target added later is covered without
        // anyone remembering to add a case here.
        for environment in [Environment::Production, Environment::Development] {
            for configured in [
                LevelFilter::ERROR,
                LevelFilter::WARN,
                LevelFilter::INFO,
                LevelFilter::DEBUG,
                LevelFilter::TRACE,
            ] {
                let mut levels = Levels::shipped(environment);
                levels.default = configured;
                let filter = levels.filter();
                let wanted = configured.into_level().expect("not OFF");
                for target in OWNED_TARGETS {
                    assert!(
                        filter.would_enable(target, &wanted),
                        "{target} is not selectable at {configured:?} in {environment:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_floor_holds_at_the_filter_and_not_only_in_the_rule() {
        // `off` is a level a config may name, and the three owned targets then
        // go quiet -- except the one that reports on another process, which is
        // exactly the target a quiet config would want when the Agent dies.
        let mut levels = Levels::shipped(Environment::Production);
        levels.default = LevelFilter::OFF;
        let filter = levels.filter();
        assert!(filter.would_enable(SUPERVISION_TARGET, &Level::INFO));
        assert!(!filter.would_enable("webview", &Level::INFO));
        assert!(!filter.would_enable("desktop.startup", &Level::INFO));
    }
}
