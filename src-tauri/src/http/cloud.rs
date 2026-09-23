//! The client for Cloud.

use reqwest::Client;

/// Talks to Cloud.
///
/// Holds no base URL and no credential: the base URL arrives as a command
/// argument and the credential comes from the node registration performed during
/// binding, so both are per-request facts rather than client state.
pub struct CloudClient {
    pub inner: Client,
}

impl CloudClient {
    pub fn new() -> Self {
        Self {
            inner: Client::new(),
        }
    }
}
