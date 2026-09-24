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
//!
//! The proxy decision is per **client**, not per request: one client either
//! carries the system proxy for every destination or for none, and there is no
//! way to narrow it to a host list (see `build_client_without_proxy`). So the
//! loopback surface holds its own client, and anything that can reach both a
//! loopback and a remote host has to pick between two — by the URL in hand, at
//! the moment it has one.

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
    timed_builder(config)
        .build()
        .expect("the validated http timeouts must produce a usable client")
}

/// A client that goes straight to its destination whatever the system says.
///
/// **All or nothing, and that is not a choice.** `reqwest` 0.12.28 has no
/// expression for "the system proxy, except for these hosts":
/// `ClientBuilder::proxy` switches the automatic system proxy *off* rather than
/// scoping it (`async_impl/client.rs:1414-1418`), `no_proxy` clears every proxy
/// and does the same (`:1428-1432`), and neither leaves a way to switch it back
/// on — `auto_sys_proxy` is only ever assigned `false` outside its default.
/// `Proxy::no_proxy` scopes a proxy you built yourself, not the system one. So a
/// client either carries the system proxy for every destination or for none, and
/// a surface that must not be proxied needs its own client.
pub(crate) fn build_client_without_proxy(config: &DesktopConfig) -> reqwest::Client {
    timed_builder(config)
        .no_proxy()
        .build()
        .expect("the validated http timeouts must produce a usable client")
}

/// The timeouts both clients share. `Client::new()` has none, so a request
/// against a sidecar that is starting, hung, or gone waits forever.
fn timed_builder(config: &DesktopConfig) -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(config.http.request_timeout_seconds))
        .connect_timeout(Duration::from_secs(config.http.connect_timeout_seconds))
}

/// Whether a request to `url` would stay on this machine.
///
/// The answer decides which client a request is handed to, and it has to be
/// worth getting right: on a machine with a system proxy configured, a loopback
/// request that transits it stops being a loopback request. Measured on this
/// machine (Clash at `127.0.0.1:7897`), a request to a **closed** loopback port
/// through the proxied client came back `502 Bad Gateway` after 3.0s, where the
/// unproxied client got `ECONNREFUSED` in 405µs — the 502 is the proxy answering
/// for a port nothing listens on, and it is indistinguishable from a real
/// upstream failure.
///
/// Parsed, not pattern-matched, and the credential case is why:
/// `http://localhost@evil.test/` contains `localhost` *and* names a remote host,
/// and only a parse separates the two.
///
/// `reqwest::Url` is a re-export of `url::Url` (`reqwest-0.12.28/src/lib.rs:280`,
/// an unconditional `pub use`), so using it adds no dependency — `url` is already
/// reqwest's own parsing entry point. It is used here rather than
/// `config::parse_http_url`, which is a private shape check that returns
/// everything after the scheme and cannot answer "what is the host?".
///
/// Two deliberate non-goals, so they read as decisions rather than oversights:
/// `0.0.0.0` is **not** loopback (`config.rs` accepts it as a legal
/// `cloud.base_url`, and on macOS it does reach this machine, so it stays
/// proxied), and IPv4-mapped IPv6 (`[::ffff:127.0.0.1]`) is not treated as
/// loopback either — nothing in this repo produces that form.
fn is_loopback_url(url: &str) -> bool {
    let Some(host) = reqwest::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_owned))
    else {
        // Not a URL, or no host at all. Nothing here can be called local, so
        // the client that keeps the system proxy is the honest choice — and it
        // is also what every destination got before this check existed.
        return false;
    };
    // `host_str` renders an IPv6 address with its brackets (`url`'s
    // `Host::Ipv6` Display writes `[`..`]`), and `"[::1]".parse::<IpAddr>()`
    // fails — without this strip an IPv6 loopback would read as remote.
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    match bare.parse::<std::net::IpAddr>() {
        // `Ipv4Addr::is_loopback` is the whole 127.0.0.0/8 and `Ipv6Addr`'s is
        // `::1` alone, which is exactly the rule wanted. `0.0.0.0` and `::` are
        // *unspecified*, not loopback, and are correctly rejected.
        Ok(address) => address.is_loopback(),
        // Exact, never a substring: `localhost.evil.test` is a remote name.
        Err(_) => bare.eq_ignore_ascii_case("localhost"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{load_with, Environment, PRODUCTION_TOML};
    use std::collections::BTreeMap;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn shipped() -> DesktopConfig {
        load_with(&BTreeMap::new(), PRODUCTION_TOML, Environment::Production)
            .expect("the shipped resource must load")
    }

    /// A listener that answers every request with `status`, counting accepts.
    ///
    /// It loops rather than serving one request so the same helper can be a
    /// destination (reached once) and a proxy (reached once by the control arm
    /// and never by the subject). The thread is deliberately not joined: the
    /// harness ends with `std::process::exit`, which does not wait for it.
    fn listener_answering(status: u16) -> (u16, Arc<AtomicUsize>) {
        let hits = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&hits);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a port");
        let port = listener.local_addr().expect("local addr").port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                counted.fetch_add(1, Ordering::SeqCst);
                // Whatever arrived is drained and thrown away: for the proxy arm
                // this is an absolute-form request line, and nothing here needs
                // to read it to know that it went somewhere it should not have.
                let mut sink = [0u8; 1024];
                let _ = stream.read(&mut sink);
                let _ = stream.write_all(
                    format!("HTTP/1.1 {status} Test\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                        .as_bytes(),
                );
                let _ = stream.flush();
            }
        });
        (port, hits)
    }

    /// Which hosts count as this machine.
    ///
    /// Both directions are asserted on purpose: a rule that answers "no" to
    /// everything passes a refutation-only test, and the same reasoning is
    /// already written down in `config.rs` for its own tables.
    #[test]
    fn only_loopback_destinations_are_treated_as_local() {
        for local in [
            "http://127.0.0.1:18080/healthz",
            "http://127.0.0.2/healthz",
            "http://127.255.255.254/healthz",
            "http://localhost:18080/healthz",
            "http://LOCALHOST/healthz",
            "http://[::1]:18080/healthz",
        ] {
            assert!(is_loopback_url(local), "{local} is this machine");
        }

        for remote in [
            // Unspecified, not loopback, and a legal `cloud.base_url`.
            "http://0.0.0.0:18080/healthz",
            "http://192.168.1.10:18080/healthz",
            "https://cloud.example.test/healthz",
            // A name that contains a local one is still a remote name.
            "http://localhost.evil.test/healthz",
            "http://127.0.0.1.evil.test/healthz",
            "http://[::2]:18080/healthz",
            // The reason this parses instead of searching for a substring: the
            // host here is `evil.test` and `localhost` is only the userinfo.
            "http://localhost@evil.test/healthz",
            // No scheme at all — `Url::parse` reads `127.0.0.1` as the scheme.
            "127.0.0.1:18080",
            "",
            "http:///healthz",
        ] {
            assert!(!is_loopback_url(remote), "{remote} is not this machine");
        }
    }

    /// The two clients differ in exactly the way their names claim.
    ///
    /// `reqwest` exposes no getter for a built client's proxy configuration, so
    /// the only observable is `Debug` — and it is enough, for a reason that is
    /// readable in the source: `ClientRef::fmt_fields` writes `proxies` only when
    /// the list is non-empty (`reqwest-0.12.28/src/async_impl/client.rs:2943`),
    /// and `build()` pushes the system matcher whenever `auto_sys_proxy` is true
    /// (`:418-421`) — **regardless of whether this machine has a proxy**. So this
    /// assertion is machine-independent, and it is what actually catches a
    /// dropped `.no_proxy()`: measured, with the call removed two tests fail —
    /// this one and the closed-port one below. The fake-proxy arm does **not**,
    /// and the reason is worth knowing: the system proxy dials the destination
    /// itself and the destination answers 200, so every assertion in that test
    /// still holds while the request is in fact leaving the machine. That test
    /// proves the bypass works when it is present; it cannot prove it is present.
    ///
    /// It does depend on a field name from a dependency. That is accepted because
    /// the failure direction is loud — a renamed field makes this red, not green.
    #[test]
    fn the_unproxied_client_is_built_without_the_system_proxy() {
        let config = shipped();
        let proxied = format!("{:?}", build_client(&config));
        let direct = format!("{:?}", build_client_without_proxy(&config));

        assert!(proxied.contains("proxies"), "the system proxy must be in the builder: {proxied}");
        assert!(!direct.contains("proxies"), "nothing may be proxied here: {direct}");
        assert_ne!(proxied, direct);
    }

    /// Bypassing is real, not just a flag on a builder.
    ///
    /// Two listeners: the destination and a proxy that answers 502 to everything.
    /// The subject is the client the loopback surface is actually built with. The
    /// control is the same request through a client whose only difference is that
    /// it was handed that proxy explicitly — it cannot be `build_client`, which
    /// would dial this machine's real system proxy and make the test depend on
    /// what is installed on the developer's laptop.
    ///
    /// The criterion is the **accept count**, not the status code: it does not
    /// care whether `hyper` forwards plain-http through the proxy in absolute
    /// form or tunnels it, and the control's `dest_hits` staying at 1 proves the
    /// traffic provably did not arrive on its own.
    #[tokio::test]
    async fn a_loopback_request_does_not_reach_the_proxy_it_was_handed() {
        let (destination, destination_hits) = listener_answering(200);
        let (proxy, proxy_hits) = listener_answering(502);
        let target = format!("http://127.0.0.1:{destination}/healthz");
        let config = shipped();
        let hits = |counter: &Arc<AtomicUsize>| counter.load(Ordering::SeqCst);

        let response = build_client_without_proxy(&config)
            .get(&target)
            .send()
            .await
            .expect("the destination is listening");
        assert_eq!(response.status(), 200);
        assert_eq!(hits(&destination_hits), 1, "the request must reach the destination");
        assert_eq!(hits(&proxy_hits), 0, "and nothing else");

        let control = reqwest::Client::builder()
            .proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{proxy}")).expect("a proxy URL"))
            .build()
            .expect("a client");
        let response = control
            .get(&target)
            .send()
            .await
            .expect("the fake proxy answers everything");
        assert_eq!(response.status(), 502, "the control must be answered by the proxy");
        assert_eq!(hits(&proxy_hits), 1, "the control went through the proxy");
        assert_eq!(hits(&destination_hits), 1, "the control never reached the destination");
    }

    /// A port with nothing behind it fails fast, and not by waiting.
    ///
    /// The counterpart to the timeout test below, and the reason a closed loopback
    /// port cannot be used to exercise `connect_timeout`: the kernel refuses the
    /// connection at once, so there is no unanswered SYN for that timeout to
    /// expire on (`connect_timeout_seconds` is set to 10 here and must not be
    /// what ends this). It also pins the other half of the proxy change: with the
    /// system proxy in the way, this request came back `502` from the proxy after
    /// ~3s instead of failing at all.
    #[tokio::test]
    async fn a_closed_loopback_port_is_refused_promptly_instead_of_hanging() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a port");
        let port = listener.local_addr().expect("local addr").port();
        drop(listener);

        let mut config = shipped();
        config.http.connect_timeout_seconds = 10;
        let client = build_client_without_proxy(&config);

        let started = std::time::Instant::now();
        let error = client
            .get(format!("http://127.0.0.1:{port}/healthz"))
            .send()
            .await
            .expect_err("nothing is listening, so this cannot succeed");
        let elapsed = started.elapsed();

        assert!(!error.is_timeout(), "a refusal is not a deadline: {error}");
        assert!(elapsed < Duration::from_secs(1), "it gave up only after {elapsed:?}");
    }

    /// A client built from the config gives up on a server that never answers.
    ///
    /// This is the whole reason `Client::new()` had to go. `reqwest`'s timeout is
    /// not readable back off a built `Client`, so the only honest way to test it
    /// is to make a request against something that is reachable and mute — the
    /// exact shape of a sidecar that is starting, or hung. Without the timeout,
    /// "the sidecar is hung" and "the sidecar never started" reach the user as
    /// the same frozen button.
    ///
    /// It uses the unproxied client because the destination is on this machine
    /// and that is the client this surface now uses. The distinction is worth
    /// stating plainly, because it is *not* something these assertions can see:
    /// measured on a machine with a system proxy, this test passed 20 times out
    /// of 20 with the proxied client too — the proxy dials the mute listener, no
    /// response comes back, and this client's own deadline fires first, which is
    /// an equally valid `is_timeout`. What the change buys is that the deadline
    /// is now measured against the server this test started rather than against
    /// a proxy standing in front of it, and that the result no longer depends on
    /// what is installed on the machine running the tests.
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
        let client = build_client_without_proxy(&config);

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
