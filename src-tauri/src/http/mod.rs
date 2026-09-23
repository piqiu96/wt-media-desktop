//! HTTP clients for the two surfaces Desktop talks to.
//!
//! They are separate types because they are trusted differently. The Local Agent
//! client targets a loopback port on this machine and carries the per-launch
//! runtime token on every request; the Cloud client is reached at a base URL the
//! caller supplies per request and authenticates with the node credential the
//! bind flow stored. One shared type for both meant a command could reach either
//! surface without saying which — and made "does this call carry the local
//! token?" unanswerable by looking at the type.
//!
//! Neither client is built with `Client::new()`. That constructor has no timeout
//! at all, so a request against a sidecar that is starting, hung, or gone waits
//! forever and the command never returns — the failure and the hang look the same
//! to the user. Both timeouts come from the config file.

mod cloud;
mod local_agent;

pub use cloud::CloudClient;
pub use local_agent::LocalAgentClient;

use crate::config::DesktopConfig;
use std::time::Duration;

/// A client with the configured timeouts.
///
/// `expect` rather than a fallback: the config has already been validated
/// (`http.request_timeout_seconds` and `http.connect_timeout_seconds` are both
/// non-zero), and a client that cannot be built is not something a launch can
/// continue past — every command on this surface would fail, with a worse
/// message than this one.
pub(crate) fn build_client(config: &DesktopConfig) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(config.http.request_timeout_seconds))
        .connect_timeout(Duration::from_secs(config.http.connect_timeout_seconds))
        .build()
        .expect("the validated http timeouts must produce a usable client")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{load_with, Environment, PRODUCTION_TOML};
    use std::collections::BTreeMap;
    use std::io::Read;

    fn shipped() -> DesktopConfig {
        load_with(&BTreeMap::new(), PRODUCTION_TOML, Environment::Production)
            .expect("the shipped resource must load")
    }

    /// A client built from the config gives up on a server that never answers.
    ///
    /// This is the whole reason `Client::new()` had to go. `reqwest`'s timeout is
    /// not readable back off a built `Client`, so the only honest way to test it
    /// is to make a request against something that is reachable and mute — the
    /// exact shape of a sidecar that is starting, or hung. Without the timeout,
    /// "the sidecar is hung" and "the sidecar never started" reach the user as
    /// the same frozen button.
    #[tokio::test]
    async fn a_request_to_a_silent_server_gives_up_instead_of_waiting_forever() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a port");
        let port = listener.local_addr().expect("local addr").port();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut sink = [0u8; 1024];
                // Read the request, answer nothing, and outlive the client's
                // timeout — otherwise the test would be measuring the server's
                // exit rather than the client's deadline.
                let _ = stream.read(&mut sink);
                std::thread::sleep(Duration::from_secs(8));
            }
        });

        let mut config = shipped();
        config.http.request_timeout_seconds = 1;
        let client = build_client(&config);

        let started = std::time::Instant::now();
        let outcome = client
            .get(format!("http://127.0.0.1:{port}/healthz"))
            .send()
            .await;
        let elapsed = started.elapsed();

        let error = outcome.expect_err("a mute server must not produce a response");
        assert!(error.is_timeout(), "the deadline must be what ended this: {error}");
        assert!(elapsed < Duration::from_secs(5), "gave up only after {elapsed:?}");
    }
}
