//! What Desktop is willing to tell the page about its own configuration.
//!
//! The page needs a few facts that only the native side knows, because they come
//! from a config file the page cannot read. It does not need — and must not get —
//! the rest: `agent.data_dir` names a directory on the user's disk, and
//! `cloud.base_url` is a URL that may carry embedded credentials.
//!
//! So this is a **closed set**, not a filtered dump of the config. That
//! distinction is the whole point, and it is enforced by enumerating the keys the
//! page receives rather than by checking that known-bad ones are absent: a
//! blocklist of private keys has to be extended every time the config grows a
//! field, and the field nobody thought to add is exactly the one that leaks.

use crate::config::{DesktopConfig, Environment};
use serde::Serialize;

/// The facts the page is allowed to know, and no others.
#[derive(Clone, Debug, Serialize)]
pub struct PublicConfig {
    /// Where Cloud is.
    ///
    /// The page used to answer this itself with a loopback literal
    /// (`web/src/apps/desktop/features/local-agent/init.js::cloudBaseUrl`),
    /// which is correct on a development machine and wrong in every deployment
    /// — including the packaged one, whose own origin is `tauri.localhost`.
    pub cloud_base_url: String,
    /// The loopback port the sidecar listens on.
    ///
    /// For display and diagnostics only. The page must never assemble a URL from
    /// it: every Local Agent call goes through a Rust command, and Vue reaching
    /// a loopback port directly is what `localAgentBoundary.test.js` forbids.
    pub local_agent_port: u16,
    /// Which environment this launch is, **after** the build's floor is applied
    /// — `DesktopConfig::environment`, not the file's own declaration. A release
    /// binary is production even if the file it read says otherwise.
    pub environment: Environment,
}

impl PublicConfig {
    /// One named field per line, so adding to the wire format is an edit to this
    /// function and to the test that enumerates its output.
    pub fn from_config(config: &DesktopConfig) -> Self {
        Self {
            cloud_base_url: config.cloud.base_url.clone(),
            local_agent_port: config.agent.port,
            environment: config.environment,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{load_with, PRODUCTION_TOML};
    use std::collections::BTreeMap;

    fn config_with_environment(environment: Environment) -> DesktopConfig {
        let text = match environment {
            Environment::Production => PRODUCTION_TOML.to_string(),
            // `python_fallback` is legal in development and forbidden in
            // production, so flipping it is what makes a development config that
            // validates — which also makes `development` a section whose value
            // differs between the two cases.
            Environment::Development => PRODUCTION_TOML
                .replace(
                    "environment = \"production\"",
                    "environment = \"development\"",
                )
                .replace("python_fallback = false", "python_fallback = true"),
        };
        load_with(&BTreeMap::new(), &text, environment).expect("the test config must load")
    }

    /// A development config in which every field the page must *not* see carries
    /// an unmistakable value. Without distinct values, "the answer does not carry
    /// it" would also hold for an answer that carries nothing at all.
    fn distinctive() -> DesktopConfig {
        let mut config = config_with_environment(Environment::Development);
        config.cloud.base_url = "https://cloud.example.test".to_string();
        config.browser.csp_connect_src = "https://cloud.example.test".to_string();
        config.agent.port = 18765;
        config.agent.data_dir = Some("/Users/example/agent-data".to_string());
        config.http.request_timeout_seconds = 4242;
        config.http.connect_timeout_seconds = 4243;
        config.sidecar.start_timeout_ms = 4244;
        config.sidecar.stop_timeout_ms = 4245;
        config
    }

    fn json_of(config: &DesktopConfig) -> serde_json::Value {
        serde_json::to_value(PublicConfig::from_config(config)).expect("the DTO must serialize")
    }

    /// The ratchet: the page receives **exactly** these three keys.
    ///
    /// Written as an equality on the sorted key set rather than as a series of
    /// `assert!(keys.get("…").is_none())`. A field added to `PublicConfig`
    /// tomorrow fails here, which is the intended cost — publishing a config
    /// field to the WebView is a decision, and this is where it gets made
    /// explicitly instead of by accident.
    #[test]
    fn the_page_gets_exactly_the_three_facts_it_is_allowed() {
        let value = json_of(&distinctive());
        let mut keys: Vec<&str> = value
            .as_object()
            .expect("the DTO must serialize to an object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();

        assert_eq!(keys, ["cloud_base_url", "environment", "local_agent_port"]);
    }

    /// And it is not a constant: each fact is the config's value, and the
    /// environment is two-sided so the fallback cannot be "whatever the default
    /// is".
    #[test]
    fn each_fact_is_read_from_the_config() {
        let development = json_of(&distinctive());
        assert_eq!(development["cloud_base_url"], "https://cloud.example.test");
        assert_eq!(development["local_agent_port"], 18765);
        assert_eq!(development["environment"], "development");

        let mut production = config_with_environment(Environment::Production);
        production.cloud.base_url = "https://other.example.test".to_string();
        production.agent.port = 8765;
        let production = json_of(&production);
        assert_eq!(production["cloud_base_url"], "https://other.example.test");
        assert_eq!(production["local_agent_port"], 8765);
        assert_eq!(production["environment"], "production");
    }

    /// No private **value** appears anywhere in the answer, checked over the
    /// whole serialized string so one smuggled under a public key is caught too.
    ///
    /// What this covers is the fields that have a value distinctive enough to
    /// search for. It deliberately says nothing about `development.
    /// python_fallback`: a boolean has only two spellings, and neither is a
    /// sentinel — `"true"` would match any future field for the wrong reason.
    /// That field's guarantee is the key set above, and only that: exposing it
    /// at all would have to expose a new key, which is where the ratchet lives.
    #[test]
    fn no_private_value_appears_anywhere_in_the_answer() {
        let text = serde_json::to_string(&PublicConfig::from_config(&distinctive()))
            .expect("the DTO must serialize");

        for private in [
            "/Users/example/agent-data", // agent.data_dir
            "4242",                      // http.request_timeout_seconds
            "4243",                      // http.connect_timeout_seconds
            "4244",                      // sidecar.start_timeout_ms
            "4245",                      // sidecar.stop_timeout_ms
        ] {
            assert!(
                !text.contains(private),
                "{private:?} must not reach the page: {text}"
            );
        }
    }
}
