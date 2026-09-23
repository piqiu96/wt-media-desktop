//! The client for the bundled Local Agent on loopback.

use reqwest::Client;

/// Talks to the Local Agent sidecar.
///
/// Holds the base URL because every call goes to the same loopback port; the
/// port comes from configuration rather than a literal.
pub struct LocalAgentClient {
    pub inner: Client,
    pub base: String,
}

impl LocalAgentClient {
    pub fn new(port: u16) -> Self {
        Self {
            inner: Client::new(),
            base: format!("http://127.0.0.1:{}", port),
        }
    }
}
