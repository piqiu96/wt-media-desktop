//! The client for Cloud.

use super::build_client;
use crate::config::DesktopConfig;
use reqwest::{Client, RequestBuilder};

/// Talks to Cloud.
///
/// Holds no base URL and no credential: the base URL arrives as a command
/// argument and the credential comes from the node registration performed during
/// binding, so both are per-request facts rather than client state. Unlike the
/// Local Agent client, this one attaches nothing by itself — a Cloud call that
/// needs a credential says so at the call site with `bearer_auth`, and the
/// registration call correctly has none.
pub struct CloudClient {
    inner: Client,
}

impl CloudClient {
    pub fn new(config: &DesktopConfig) -> Self {
        Self { inner: build_client(config) }
    }

    /// POST to a fully-qualified URL. Unlike the local client there is no base
    /// to join: Cloud's address is a command argument, not launch state.
    pub fn post(&self, url: &str) -> RequestBuilder {
        self.inner.post(url)
    }
}
