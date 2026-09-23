//! The client for the bundled Local Agent on loopback.

use super::build_client;
use crate::config::DesktopConfig;
use crate::token::RuntimeToken;
use reqwest::{Client, RequestBuilder};

/// Talks to the Local Agent sidecar.
///
/// Holds the base URL because every call goes to the same loopback host and
/// port, and the runtime token because every call has to present it. The Agent
/// makes the token mandatory on *every* GET and POST the moment it is given one
/// (`local_api/server.py::_check_auth`: an empty token disables the check, a
/// non-empty one requires `Bearer <token>`); there is no per-route exception, so
/// a call that forgets it is a 401 and nothing else.
///
/// The token has one owner — this type — and both of its readers go through it:
/// `get`/`post` put it on the header, and `token` hands it to the sidecar's
/// environment. Two holders would be a way for the Agent to be told one secret
/// and challenged with another.
pub struct LocalAgentClient {
    inner: Client,
    base: String,
    token: RuntimeToken,
}

impl LocalAgentClient {
    pub fn new(config: &DesktopConfig, token: RuntimeToken) -> Self {
        Self {
            inner: build_client(config),
            base: format!("http://{}:{}", config.agent.host, config.agent.port),
            token,
        }
    }

    /// The token this launch generated. For the sidecar's environment, which is
    /// the only other place the value is allowed to appear.
    pub fn token(&self) -> &RuntimeToken {
        &self.token
    }

    pub fn get(&self, path: &str) -> RequestBuilder {
        self.bearer(self.inner.get(self.url(path)))
    }

    pub fn post(&self, path: &str) -> RequestBuilder {
        self.bearer(self.inner.post(self.url(path)))
    }

    /// Absolute, because the sidecar's port comes from configuration and a
    /// relative URL would silently pick up whatever base the caller had.
    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    fn bearer(&self, request: RequestBuilder) -> RequestBuilder {
        request.bearer_auth(self.token.expose())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{load_with, Environment, PRODUCTION_TOML};
    use std::collections::BTreeMap;

    fn config_with(host: &str, port: u16) -> DesktopConfig {
        let text = PRODUCTION_TOML
            .replace("host = \"127.0.0.1\"", &format!("host = {:?}", host))
            .replace("port = 8765", &format!("port = {}", port));
        load_with(&BTreeMap::new(), &text, Environment::Production).expect("test config")
    }

    /// The base URL is built from the config, both halves of it — the port is
    /// what `main.rs` used to hard-code, and the host is the one validation
    /// restricts to loopback.
    #[test]
    fn the_base_url_comes_from_the_config() {
        let client = LocalAgentClient::new(&config_with("127.0.0.1", 18999), RuntimeToken::generate());
        assert_eq!(client.url("/healthz"), "http://127.0.0.1:18999/healthz");

        let client = LocalAgentClient::new(&config_with("localhost", 8765), RuntimeToken::generate());
        assert_eq!(client.url("/api/v1/status"), "http://localhost:8765/api/v1/status");
    }

    /// Every request carries its own client's token — the one `token()` hands to
    /// the sidecar. A client that built the header from a different value would
    /// authenticate against a secret the Agent was never told, and every call
    /// would 401.
    ///
    /// Two clients with two different tokens, not one: with a single client the
    /// assertion also holds for a client that reads some other token, or a
    /// constant, since there would be nothing for it to disagree with.
    #[test]
    fn every_request_carries_its_own_clients_token() {
        let first = RuntimeToken::generate();
        let second = RuntimeToken::generate();
        let one = LocalAgentClient::new(&config_with("127.0.0.1", 8765), first.clone());
        let other = LocalAgentClient::new(&config_with("127.0.0.1", 8765), second.clone());

        let header_of = |request: RequestBuilder| {
            request
                .build()
                .expect("the request must build")
                .headers()
                .get(reqwest::header::AUTHORIZATION)
                .expect("a local-agent request without the token is a 401")
                .to_str()
                .expect("the header must be readable")
                .to_string()
        };

        for (client, token) in [(&one, &first), (&other, &second)] {
            let expected = format!("Bearer {}", token.expose());
            assert_eq!(header_of(client.get("/healthz")), expected);
            assert_eq!(header_of(client.post("/api/v1/bind")), expected);
        }
        assert_ne!(first.expose(), second.expose(), "the two cases must differ to mean anything");
    }
}
