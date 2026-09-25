//! Desktop's configuration: one schema, one parse path, and two overlay rules
//! that can only ever move in the stricter direction.
//!
//! 1. `Environment::Production` ignores the **whole** `WT_MEDIA_DESKTOP_*`
//!    namespace. A blocklist of "address-shaped" keys would have to grow with
//!    every key ever added, and `WT_MEDIA_DESKTOP_CONFIG` — the file locator —
//!    sits outside such a list by construction.
//! 2. A release build is always `Production`, and a file may not talk its way
//!    out of it. `Environment` derives `Ord` with `Development < Production`
//!    precisely so "the stricter of the build and the file wins" is expressed
//!    as `max` rather than as a hand-written branch that a later edit can get
//!    wrong.
//!
//! `load_with` is pure — env map, file text, environment in; config out — so
//! both rules are testable without touching the real environment or the
//! filesystem. Reading the file from the right place on disk (dev vs bundled)
//! is a separate, impure concern that belongs to the loader, not here.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The production resource, compiled in. A bundle whose resource file is
/// missing or corrupt falls back to this rather than starting with no CSP and
/// no Cloud address.
pub const PRODUCTION_TOML: &str = include_str!("../resources/desktop.production.toml");

/// The only override a non-production environment honours. Named after the
/// variable it replaces, so an existing development shell keeps working.
pub const ENV_PYTHON_FALLBACK: &str = "WT_MEDIA_DESKTOP_ALLOW_PYTHON_FALLBACK";

/// `Serialize` exists for exactly one reader: `dto::PublicConfig` hands the page
/// the effective environment, and the wire value has to be the same lowercase
/// word the file uses (`"development"` / `"production"`) — `rename_all` covers
/// both directions so the two can never spell it differently.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Environment {
    Development,
    Production,
}

impl Environment {
    /// The same lowercase word, as a `&str`.
    ///
    /// `PublicConfig` puts the enum itself on the wire and serde writes this
    /// word; a document this crate builds by hand (the diagnostic bundle) needs
    /// the word without a serializer in the middle. `code_agrees_with_serde`
    /// holds the two together, so a change to one that missed the other fails
    /// there rather than in an archive somebody has already sent.
    pub const fn code(self) -> &'static str {
        match self {
            Environment::Development => "development",
            Environment::Production => "production",
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopConfig {
    pub environment: Environment,
    pub agent: Agent,
    pub cloud: Cloud,
    pub browser: Browser,
    pub http: Http,
    pub sidecar: Sidecar,
    pub logging: Logging,
    pub development: Development,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Agent {
    /// Loopback only: the sidecar is a per-user local process, and reaching it
    /// over any other interface would expose the local API to the network.
    pub host: String,
    pub port: u16,
    /// `None` means "the Agent resolves its own default".
    #[serde(default)]
    pub data_dir: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cloud {
    pub base_url: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Browser {
    pub csp_connect_src: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Http {
    pub request_timeout_seconds: u64,
    pub connect_timeout_seconds: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sidecar {
    pub start_timeout_ms: u64,
    pub stop_timeout_ms: u64,
}

/// The spellings `logging.level` accepts, in the order the rejection message
/// lists them.
///
/// Exported because the subscriber has to map the accepted token onto a
/// `LevelFilter`, and a second hand-written list there would be free to accept
/// a level this one rejects. Lowercase only: one spelling per level, so a file
/// cannot say `INFO` here and `info` in the message about it.
pub const LOG_LEVELS: [&str; 6] = ["off", "error", "warn", "info", "debug", "trace"];

/// `logging.level = "auto"` means "whatever the environment ships" — production
/// INFO, development DEBUG (ruling 四). It is a token rather than an absent key
/// because a comment cannot be validated, and `#[serde(default)]` here would let
/// the shipped file and this struct drift apart without a word.
pub const LOG_LEVEL_AUTO: &str = "auto";

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Logging {
    /// `auto` or one of [`LOG_LEVELS`].
    pub level: String,
    /// Days a rolled file survives before the age sweep deletes it.
    ///
    /// Signed, because TOML has negative integers and `-1` parses into `i64`
    /// without complaint; a negative retention would put the cutoff in the
    /// future and expire every file including today's, so validation rejects
    /// the sign rather than only zero.
    pub retention_days: i64,
    /// How long a **single record** may be before it is cut and marked.
    ///
    /// This is the one size left in `[logging]`. The per-file cap and the
    /// directory budget were both cancelled by the user's ruling of 2026-09-24
    /// (CHG-058 D-03, 「不需要控制总量，只需要控制能保留多少天」): rotation is
    /// what bounds a file's size now, and it does so by the hour, not by a byte
    /// count. A record is the one thing rotation cannot bound -- one runaway
    /// line would still grow the live file forever, and no rotation would help
    /// because the file it is growing is the one being written.
    pub max_record_bytes: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Development {
    pub python_fallback: bool,
}

#[derive(Debug)]
pub enum ConfigError {
    Parse(String),
    Invalid(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Parse(message) => write!(f, "invalid desktop config: {}", message),
            ConfigError::Invalid(message) => write!(f, "unusable desktop config: {}", message),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Parse `file_text`, overlay `env` where allowed, and validate the result.
///
/// `environment` is the environment the caller determined from the build (a
/// release binary passes `Production` unconditionally). The effective
/// environment is the stricter of that and whatever the file declares.
pub fn load_with(
    env: &BTreeMap<String, String>,
    file_text: &str,
    environment: Environment,
) -> Result<DesktopConfig, ConfigError> {
    let mut config: DesktopConfig = toml::from_str(file_text).map_err(parse_error)?;
    config.environment = config.environment.max(environment);

    // Rule 1. Production consults no environment variable at all — including
    // ones added later, and including the file locator, which the caller
    // resolves before ever getting here.
    if config.environment == Environment::Development {
        // Rule for the one key that is allowed to differ: presence overrides
        // the file, absence leaves it alone. The accepted value is `1`, matching
        // the variable this key replaces.
        if let Some(value) = env.get(ENV_PYTHON_FALLBACK) {
            config.development.python_fallback = value == "1";
        }
    }

    config.validate()?;
    Ok(config)
}

impl DesktopConfig {
    /// The checks the plan fixes: a loopback Agent host, a port outside the
    /// privileged range, non-zero timeouts, and — in production — a parseable
    /// Cloud address and no Python fallback. Plus two additions noted where
    /// they appear: the sidecar start timeout counts as a timeout, and a
    /// production `connect-src` must not be empty.
    ///
    /// Every message names the offending **key** and never its value. The values
    /// here are mostly innocuous, but `cloud.base_url` is a URL that may carry
    /// embedded credentials, and these messages reach logs — so the rule is
    /// applied uniformly rather than per-field, where it would have to be
    /// re-justified on every edit. Same rule as D-07's TOML credential keys.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if !is_loopback_host(&self.agent.host) {
            return Err(ConfigError::Invalid(
                "agent.host must be loopback (127.0.0.1, localhost or ::1)".to_string(),
            ));
        }
        if self.agent.port < 1024 {
            // 65535 is already the type's ceiling.
            return Err(ConfigError::Invalid(
                "agent.port must be >= 1024".to_string(),
            ));
        }
        for (key, seconds) in [
            (
                "http.request_timeout_seconds",
                self.http.request_timeout_seconds,
            ),
            (
                "http.connect_timeout_seconds",
                self.http.connect_timeout_seconds,
            ),
        ] {
            if seconds == 0 {
                return Err(ConfigError::Invalid(format!("{} must be > 0", key)));
            }
        }
        // Both ends of the Agent's life, checked the same way. Zero is the only
        // value the type cannot reject and the only one that would break the
        // wait: a zero start timeout fails a launch that was going to succeed,
        // and a zero stop timeout turns the ask into a kill with the grace
        // window skipped entirely -- the reading CHG-059 T-03 exists to tell
        // apart from the one where the Agent left on its own. Neither is a
        // "faster" setting; both are a different behaviour.
        for (key, ms) in [
            ("sidecar.start_timeout_ms", self.sidecar.start_timeout_ms),
            ("sidecar.stop_timeout_ms", self.sidecar.stop_timeout_ms),
        ] {
            if ms == 0 {
                return Err(ConfigError::Invalid(format!("{} must be > 0", key)));
            }
        }
        if self.logging.level != LOG_LEVEL_AUTO
            && !LOG_LEVELS.contains(&self.logging.level.as_str())
        {
            return Err(ConfigError::Invalid(format!(
                "logging.level must be one of {}, {}",
                LOG_LEVEL_AUTO,
                LOG_LEVELS.join(", ")
            )));
        }
        // Two checks, one per type: `retention_days` is signed and `<= 0` is
        // what that makes meaningful; `max_record_bytes` is unsigned, so zero is
        // the only value its type cannot reject and the only one the truncation
        // arithmetic could not work with.
        if self.logging.max_record_bytes == 0 {
            return Err(ConfigError::Invalid(
                "logging.max_record_bytes must be > 0".to_string(),
            ));
        }
        if self.logging.retention_days <= 0 {
            return Err(ConfigError::Invalid(
                "logging.retention_days must be > 0".to_string(),
            ));
        }
        if self.environment == Environment::Production {
            if parse_http_url(&self.cloud.base_url).is_none() {
                return Err(ConfigError::Invalid(
                    "cloud.base_url must be an http(s) URL".to_string(),
                ));
            }
            // Checked beyond the plan's list: an empty `connect-src` leaves the
            // window able to reach `'self'` only, so Cloud becomes unreachable
            // by CSP with no other symptom. Presence only — a value naming the
            // wrong origin or port still passes.
            if self.browser.csp_connect_src.trim().is_empty() {
                return Err(ConfigError::Invalid(
                    "browser.csp_connect_src must name at least one origin in production"
                        .to_string(),
                ));
            }
            if self.development.python_fallback {
                return Err(ConfigError::Invalid(
                    "development.python_fallback must be false in production".to_string(),
                ));
            }
        }
        Ok(())
    }
}

/// Keep only the toml crate's span-free message.
///
/// `toml::de::Error`'s `Display` renders the offending source line verbatim
/// (`src/de/error.rs`: `write!(f, "{line_num} | "); writeln!(f, "{content}")`),
/// so a rejected credential-looking key would have its value printed back at us
/// — and from there into a log. The message alone names the key, never the
/// value.
fn parse_error(error: toml::de::Error) -> ConfigError {
    ConfigError::Parse(error.message().to_string())
}

fn is_loopback_host(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

/// Whether `value` looks like an `http`/`https` URL with something after the
/// scheme.
///
/// This is a shape check, not a full parse: it does not validate the authority,
/// reject credentials embedded in the URL, or normalise anything. That is
/// enough for what it guards — a config that forgot the scheme, or left the key
/// blank — and avoids pulling a URL parser in as a direct dependency to answer
/// one question.
fn parse_http_url(value: &str) -> Option<&str> {
    let rest = value
        .strip_prefix("http://")
        .or_else(|| value.strip_prefix("https://"))?;
    let rest = rest.trim();
    if rest.is_empty() || rest.contains(char::is_whitespace) {
        return None;
    }
    Some(rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hand-written word and the serialized one are the same word.
    ///
    /// Two spellings of one value is the shape of bug that only shows up in a
    /// document somebody has already sent, so the agreement is asserted rather
    /// than intended. `serde_json` is used because it is the format the two
    /// readers that matter here (the page's config and the bundle's summary) are
    /// both written in.
    #[test]
    fn code_agrees_with_serde() {
        for environment in [Environment::Development, Environment::Production] {
            let serialized = serde_json::to_value(environment).expect("a serializable enum");
            assert_eq!(
                serialized,
                serde_json::Value::String(environment.code().to_string()),
                "{environment:?} spells itself two ways"
            );
        }
    }

    fn env_of(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// A minimal development config; tests mutate one key at a time so a
    /// failure names the rule it broke.
    fn development_toml() -> String {
        PRODUCTION_TOML.replace(
            "environment = \"production\"",
            "environment = \"development\"",
        )
    }

    fn development_config() -> DesktopConfig {
        load_with(
            &BTreeMap::new(),
            &development_toml(),
            Environment::Development,
        )
        .expect("the development twin of the shipped resource must load")
    }

    /// The shipped resource and this module's schema must not drift apart: a
    /// key that validation rejects would mean the bundled default is unusable.
    #[test]
    fn shipped_production_resource_parses_and_validates() {
        let config = load_with(&BTreeMap::new(), PRODUCTION_TOML, Environment::Production)
            .expect("the shipped production resource must parse and validate");

        assert_eq!(config.environment, Environment::Production);
        assert_eq!(config.agent.host, "127.0.0.1");
        assert_eq!(config.agent.port, 8765);
        assert!(config.agent.data_dir.is_none());
        assert!(!config.development.python_fallback);
    }

    /// The shipped file's two log numbers are the writer's shipped limits —
    /// the *same* two, not two that happen to agree today.
    ///
    /// Two sources, one behaviour: `rolling::Limits::SHIPPED` is what the writer
    /// uses when nobody hands it a set, and the TOML is what the app hands it
    /// once startup wires the two together. Neither reads the other, so without
    /// this the two can drift silently and the app keeps its history for a
    /// number of days that appears in no document.
    #[test]
    fn the_shipped_logging_values_are_the_writers_shipped_limits() {
        let config = load_with(&BTreeMap::new(), PRODUCTION_TOML, Environment::Production)
            .expect("the shipped production resource must load");
        let limits = crate::logging::rolling::Limits::SHIPPED;

        assert_eq!(config.logging.retention_days, limits.retention_days);
        assert_eq!(config.logging.max_record_bytes, limits.max_record_bytes);

        // And the level is pinned to the sentinel: a shipped file naming a level
        // outright would silently override ruling 四 for every install, which is
        // exactly the decision the sentinel exists to keep out of the file.
        assert_eq!(config.logging.level, LOG_LEVEL_AUTO);
    }

    /// `deny_unknown_fields` is what stops a credential from living in a file
    /// that ships with the app — and the error must name the key without
    /// repeating its value.
    ///
    /// Every section is listed, because the attribute is written **per struct**:
    /// the top-level derive guards the top-level keys and nothing else. Measured:
    /// dropping it from `Logging` alone survived the whole module, so a stray
    /// `token = "…"` under `[logging]` would have been silently ignored by the
    /// very file T-14 was adding.
    #[test]
    fn unknown_key_is_rejected_by_name_without_echoing_its_value() {
        let sections = [
            ("the top level", String::new()),
            ("agent", "[agent]\n".to_string()),
            ("cloud", "[cloud]\n".to_string()),
            ("browser", "[browser]\n".to_string()),
            ("http", "[http]\n".to_string()),
            ("sidecar", "[sidecar]\n".to_string()),
            ("logging", "[logging]\n".to_string()),
            ("development", "[development]\n".to_string()),
        ];

        for (section, header) in sections {
            let text = if header.is_empty() {
                format!("runtime_token = \"do-not-log-me\"\n{PRODUCTION_TOML}")
            } else {
                PRODUCTION_TOML.replace(
                    &header,
                    &format!("{header}runtime_token = \"do-not-log-me\"\n"),
                )
            };

            let error = match load_with(&BTreeMap::new(), &text, Environment::Production) {
                Err(error) => error,
                Ok(config) => panic!("an unknown key in {section} must be rejected: {config:?}"),
            };

            let rendered = error.to_string();
            assert!(rendered.contains("runtime_token"), "{section}: {rendered}");
            assert!(!rendered.contains("do-not-log-me"), "{section}: {rendered}");
        }
    }

    #[test]
    fn malformed_and_incomplete_toml_are_rejected() {
        let malformed = load_with(
            &BTreeMap::new(),
            "[agent\nport = ",
            Environment::Development,
        );
        assert!(
            matches!(malformed, Err(ConfigError::Parse(_))),
            "{malformed:?}"
        );

        let missing_section = PRODUCTION_TOML.replace("[cloud]", "[butterfly]");
        let incomplete = load_with(&BTreeMap::new(), &missing_section, Environment::Production);
        assert!(
            matches!(incomplete, Err(ConfigError::Parse(_))),
            "{incomplete:?}"
        );
    }

    /// Rule 1. The point of testing a *whole-namespace* rule with a key that
    /// already exists is that a future blocklist-based rewrite still passes for
    /// today's keys; the locator is the key a blocklist would have missed.
    #[test]
    fn production_ignores_the_whole_env_namespace() {
        let env = env_of(&[
            (ENV_PYTHON_FALLBACK, "1"),
            ("WT_MEDIA_DESKTOP_CONFIG", "/tmp/somebody-elses.toml"),
            ("WT_MEDIA_DESKTOP_AGENT_PORT", "9"),
        ]);

        let config = load_with(&env, PRODUCTION_TOML, Environment::Production)
            .expect("production must load with the environment present but ignored");

        assert!(!config.development.python_fallback);
        assert_eq!(config.agent.port, 8765);
    }

    #[test]
    fn development_honours_the_python_fallback_opt_in() {
        let on = load_with(
            &env_of(&[(ENV_PYTHON_FALLBACK, "1")]),
            &development_toml(),
            Environment::Development,
        )
        .expect("development must load");
        assert!(on.development.python_fallback);

        // Absence leaves the file's value alone rather than forcing `false`.
        let absent = development_config();
        assert!(!absent.development.python_fallback);

        // Any other value is off, matching the `Ok("1")` test it replaces.
        let off = load_with(
            &env_of(&[(ENV_PYTHON_FALLBACK, "true")]),
            &development_toml(),
            Environment::Development,
        )
        .expect("development must load");
        assert!(!off.development.python_fallback);
    }

    /// Rule 2, both directions: a release build cannot be talked down to
    /// development, and a file cannot be talked up out of it.
    #[test]
    fn environment_can_only_be_narrowed_never_widened() {
        let released = load_with(
            &env_of(&[(ENV_PYTHON_FALLBACK, "1")]),
            &development_toml(),
            Environment::Production,
        )
        .expect("a development file under a production build must still load");
        assert_eq!(released.environment, Environment::Production);
        assert!(
            !released.development.python_fallback,
            "the environment namespace must be ignored once the effective environment is production"
        );

        let declared = load_with(
            &env_of(&[(ENV_PYTHON_FALLBACK, "1")]),
            PRODUCTION_TOML,
            Environment::Development,
        )
        .expect("a production file under a development build must still load");
        assert_eq!(declared.environment, Environment::Production);
        assert!(!declared.development.python_fallback);
    }

    /// Both directions of the host rule in one place, because a rule that only
    /// ever says "no" is indistinguishable from a rule that rejects everything.
    #[test]
    fn agent_host_rule_accepts_loopback_and_rejects_everything_else() {
        let with_host = |host: &str| {
            let text =
                PRODUCTION_TOML.replace("host = \"127.0.0.1\"", &format!("host = {:?}", host));
            load_with(&BTreeMap::new(), &text, Environment::Production)
        };

        for host in ["127.0.0.1", "localhost", "::1"] {
            let config =
                with_host(host).unwrap_or_else(|e| panic!("host {host:?} must be accepted: {e}"));
            assert_eq!(config.agent.host, host);
        }

        for host in [
            "0.0.0.0",
            "192.168.1.10",
            "example.com",
            "",
            "127.0.0.1.evil.test",
        ] {
            let result = with_host(host);
            assert!(
                matches!(result, Err(ConfigError::Invalid(_))),
                "host {host:?} must be rejected, got {result:?}"
            );
        }
    }

    /// Each row breaks exactly one requirement; the table keeps a new check
    /// from being added without a case that proves it fires.
    ///
    /// The fourth column is the key the message must name, and it is load
    /// bearing: without it a row also passes when an *earlier* check happens to
    /// reject the rewritten file, so the row would stop proving its own rule
    /// the moment the checks are reordered or a new one is inserted above it.
    #[test]
    fn out_of_range_values_are_rejected() {
        let rows: [(&str, &str, &str, &str); 12] = [
            ("privileged port", "port = 8765", "port = 80", "agent.port"),
            (
                "zero request timeout",
                "request_timeout_seconds = 30",
                "request_timeout_seconds = 0",
                "http.request_timeout_seconds",
            ),
            (
                "zero connect timeout",
                "connect_timeout_seconds = 10",
                "connect_timeout_seconds = 0",
                "http.connect_timeout_seconds",
            ),
            (
                "zero sidecar start timeout",
                "start_timeout_ms = 15000",
                "start_timeout_ms = 0",
                "sidecar.start_timeout_ms",
            ),
            (
                "zero sidecar stop timeout",
                "stop_timeout_ms = 5000",
                "stop_timeout_ms = 0",
                "sidecar.stop_timeout_ms",
            ),
            (
                "cloud url without a scheme",
                "base_url = \"http://127.0.0.1:18080\"",
                "base_url = \"127.0.0.1:18080\"",
                "cloud.base_url",
            ),
            (
                "python fallback in production",
                "python_fallback = false",
                "python_fallback = true",
                "development.python_fallback",
            ),
            (
                "empty csp connect-src in production",
                "csp_connect_src = \"ipc: http://ipc.localhost http://127.0.0.1:18080\"",
                "csp_connect_src = \"\"",
                "browser.csp_connect_src",
            ),
            (
                "unusable log level",
                "level = \"auto\"",
                "level = \"verbose\"",
                "logging.level",
            ),
            (
                "zero record cap",
                "max_record_bytes = 1048576",
                "max_record_bytes = 0",
                "logging.max_record_bytes",
            ),
            (
                "zero log retention",
                "retention_days = 14",
                "retention_days = 0",
                "logging.retention_days",
            ),
            (
                "negative log retention",
                "retention_days = 14",
                "retention_days = -1",
                "logging.retention_days",
            ),
        ];

        for (case, from, to, key) in rows {
            assert!(
                PRODUCTION_TOML.contains(from),
                "row {case:?} matches nothing"
            );
            let text = PRODUCTION_TOML.replace(from, to);
            let result = load_with(&BTreeMap::new(), &text, Environment::Production);
            let error = match result {
                Err(ConfigError::Invalid(message)) => message,
                other => panic!("{case} must be rejected as invalid, got {other:?}"),
            };
            assert!(error.contains(key), "{case} must name {key}, got {error:?}");
        }
    }

    /// The other direction of the level rule, and the reason `LOG_LEVELS` is a
    /// constant rather than a string baked into one message: every spelling the
    /// message offers must actually load.
    ///
    /// A rejection list with no case that passes cannot be told apart from a
    /// list with a typo in it, and the message is where a user goes to find out
    /// what they are allowed to write — so it has to name all of them too.
    #[test]
    fn every_accepted_log_level_loads_and_is_named_in_the_message() {
        for level in LOG_LEVELS
            .iter()
            .copied()
            .chain(std::iter::once(LOG_LEVEL_AUTO))
        {
            let text = PRODUCTION_TOML.replace("level = \"auto\"", &format!("level = {level:?}"));
            let config = load_with(&BTreeMap::new(), &text, Environment::Production)
                .unwrap_or_else(|e| panic!("level {level:?} must be accepted: {e}"));
            assert_eq!(config.logging.level, level);
        }

        let rejected = load_with(
            &BTreeMap::new(),
            &PRODUCTION_TOML.replace("level = \"auto\"", "level = \"verbose\""),
            Environment::Production,
        )
        .expect_err("`verbose` is not a level");
        let message = match rejected {
            ConfigError::Invalid(message) => message,
            other => panic!("`verbose` must be rejected as invalid, got {other:?}"),
        };
        for level in LOG_LEVELS
            .iter()
            .copied()
            .chain(std::iter::once(LOG_LEVEL_AUTO))
        {
            assert!(
                message.contains(level),
                "the message must offer {level:?}: {message}"
            );
        }
    }

    /// The list itself, spelled out.
    ///
    /// Both directions of the rule — what loads and what the message offers —
    /// read `LOG_LEVELS`, so a mutation that shrinks the array keeps every other
    /// case in this module green while the app silently loses a level. Measured:
    /// dropping `trace` survived the whole module until this test existed. A
    /// constant that all the assertions are derived from has to be pinned
    /// somewhere by hand, exactly as `rolling::Limits::SHIPPED` is.
    ///
    /// Compared as joined text rather than as an array, so a mutation here is a
    /// failing assertion rather than a compile error — a mutation that cannot
    /// build proves nothing about behaviour.
    #[test]
    fn the_accepted_log_levels_are_these_six_spellings() {
        assert_eq!(LOG_LEVELS.len(), 6);
        assert_eq!(LOG_LEVELS.join(" "), "off error warn info debug trace");

        // The sentinel is a word, not a level: it must not also be in the list,
        // or `auto` would be read as the literal level `auto` by anything that
        // maps the list onto a `LevelFilter`.
        assert!(!LOG_LEVELS.contains(&LOG_LEVEL_AUTO));
    }

    /// The other half of the "name, never value" rule that
    /// `unknown_key_...` covers for the parse path: a *validation* error must
    /// not print the URL it rejected either, because a base URL can carry
    /// `user:secret@`.
    #[test]
    fn rejected_values_are_not_echoed_in_validation_errors() {
        let text = PRODUCTION_TOML.replace(
            "base_url = \"http://127.0.0.1:18080\"",
            "base_url = \"wt-user:wt-secret@127.0.0.1:18080\"",
        );

        let error = load_with(&BTreeMap::new(), &text, Environment::Production)
            .expect_err("a scheme-less URL must be rejected");

        let rendered = error.to_string();
        assert!(rendered.contains("cloud.base_url"), "{rendered}");
        assert!(!rendered.contains("wt-secret"), "{rendered}");
    }

    /// The development path is what the empty-Cloud-address error path relies
    /// on, so an empty address must load in development and fail in production.
    ///
    /// The development twin is built by rewriting the `environment` key, not by
    /// passing `Development` to a file that declares production: rule 2 lets the
    /// file narrow, so passing `Development` alone would still land in
    /// production and this case would pass for the wrong reason.
    #[test]
    fn empty_cloud_address_is_development_only() {
        let text =
            development_toml().replace("base_url = \"http://127.0.0.1:18080\"", "base_url = \"\"");

        let development = load_with(&BTreeMap::new(), &text, Environment::Development);
        assert!(development.is_ok(), "{development:?}");
        assert_eq!(
            development.unwrap().environment,
            Environment::Development,
            "the case must exercise the development path to mean anything"
        );

        let production = load_with(&BTreeMap::new(), &text, Environment::Production);
        assert!(
            matches!(production, Err(ConfigError::Invalid(_))),
            "{production:?}"
        );
    }
}
