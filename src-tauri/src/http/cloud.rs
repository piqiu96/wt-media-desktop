//! The client for Cloud.

use super::{build_client, build_client_without_proxy, is_loopback_url};
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
///
/// Which proxy a request uses is a third per-request fact, and it is decided at
/// the same moment and for the same reason as the URL: from the URL. Building
/// from `config.cloud.base_url` instead would be wrong, because that is not
/// reliably where requests go — `require_cloud_base_url` only trims and rejects
/// empty, it does not compare against the configured value, and the shipped
/// default *is* `http://127.0.0.1:18080` while the front end has two producers
/// that disagree about it (one returns `PublicConfig.cloud_base_url`, which may
/// be remote; others hard-code a loopback literal). So today nearly every Cloud
/// call is a loopback call, and before this each of them left the machine
/// through the system proxy.
///
/// The split costs a second connection pool and resolver. `main.rs` builds one
/// of these per launch, and `reqwest` offers no finer granularity than a client,
/// so this is the smallest thing that can be right.
pub struct CloudClient {
    proxied: Client,
    direct: Client,
}

impl CloudClient {
    pub fn new(config: &DesktopConfig) -> Self {
        Self {
            proxied: build_client(config),
            direct: build_client_without_proxy(config),
        }
    }

    /// POST to a fully-qualified URL. Unlike the local client there is no base
    /// to join: Cloud's address is a command argument, not launch state.
    pub fn post(&self, url: &str) -> RequestBuilder {
        self.client_for(url).post(url)
    }

    /// The only place the proxy decision is made. Every Cloud URL is built by a
    /// caller and every one of them is sent through `post`, so there is no
    /// second path to keep in step.
    fn client_for(&self, url: &str) -> &Client {
        if is_loopback_url(url) {
            &self.direct
        } else {
            &self.proxied
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{load_with, Environment, PRODUCTION_TOML};
    use std::collections::BTreeMap;

    fn client() -> CloudClient {
        let config = load_with(&BTreeMap::new(), PRODUCTION_TOML, Environment::Production)
            .expect("the shipped resource must load");
        CloudClient::new(&config)
    }

    /// A Cloud URL that stays on this machine skips the system proxy, and one
    /// that leaves it keeps it.
    ///
    /// This test locks the **choice**; the tests in `http` lock the two builders
    /// it chooses between. Neither substitutes for the other — a correct pair of
    /// clients wired to the wrong branch is exactly the shape of the bug this
    /// change fixes. Pointer identity is used because it is exact: there is no
    /// string to match and nothing to normalise away.
    #[test]
    fn a_loopback_cloud_url_is_sent_without_the_system_proxy() {
        let cloud = client();

        for local in ["http://127.0.0.1:18080/api/v1/nodes", "http://localhost:18080/api/v1/nodes"] {
            assert!(
                std::ptr::eq(cloud.client_for(local), &cloud.direct),
                "{local} is on this machine and must not be proxied"
            );
        }

        for remote in ["https://cloud.example.test/api/v1/nodes", "http://192.168.1.10:18080/api/v1/nodes"] {
            assert!(
                std::ptr::eq(cloud.client_for(remote), &cloud.proxied),
                "{remote} leaves this machine and keeps the system proxy"
            );
        }
    }
}
