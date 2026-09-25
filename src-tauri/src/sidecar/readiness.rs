//! Waiting for a freshly spawned Agent to be worth talking to.
//!
//! [`super::start`] returns as soon as the child process exists, which is not
//! the same moment the Agent can answer anything. Before this module the caller
//! had no way to tell the two apart: `sidecar.start_timeout_ms` was parsed,
//! validated and exported to the frontend, and **read by nothing** — the key
//! described a wait that no code performed. Every command issued in the gap
//! failed, and what a user saw was an Agent that "would not start" a few seconds
//! after one that started fine.
//!
//! Two phases, because one is not enough to be true:
//!
//! 1. **The announcement.** The Agent prints
//!    `wt-media-agent local API listening on <host>:<port>` between constructing
//!    its `ThreadingHTTPServer` — which binds *and* listens — and entering
//!    `serve_forever()`. Seeing that line means the port is open now, not that
//!    the process merely exists. The port it names is checked against the port
//!    this machine is going to call: a mismatch is a definite misconfiguration,
//!    and it is reported as such instead of as a timeout.
//! 2. **The answer.** The announcement is the Agent's own claim about itself.
//!    The second phase asks `/healthz` and requires a 2xx, which is the only
//!    reading that also covers the credential: `/healthz` sits behind the same
//!    `_check_auth` as every other route (`local_api/server.py:448`), so a
//!    sidecar told one token and challenged with another answers **401**. That
//!    drift is the failure mode [`super::environment`] documents and no build
//!    step catches, and it is fatal on sight: waiting cannot change a credential,
//!    and treating it as "ready" would start an app whose every later call 401s.
//!
//! What this does **not** cover:
//!
//! - A reworded announcement. The prefix is matched literally against a string
//!   in another repository, so an edit there makes phase one never fire and every
//!   start fails at the deadline with the "never announced" message rather than
//!   a wrong one. `wt-media-agent`'s `tests/test_sidecar_entry.py` pins the
//!   format from its side so the edit fails there first.
//! - A port announced as something that does not parse as a number is treated as
//!   no announcement at all — the same loud-but-blunt failure as a reworded line.
//! - The announcement can be evicted: only the newest [`drain::CAPACITY`] lines
//!   are held, so an Agent that prints more than that before becoming ready is
//!   read as never having announced.

use crate::config::DesktopConfig;
use crate::http::LocalAgentClient;
use crate::sidecar::drain;
use crate::state::SidecarLog;
use std::time::{Duration, Instant};

/// The line the Agent prints once its socket is listening.
///
/// **The Desktop half of a contract.** The other half is
/// `wt-media-agent/src/wt_media_agent/local_api/server.py:627`:
/// `print(f"wt-media-agent local API listening on {host}:{port}", flush=True)`.
/// Nothing checks the two halves against each other at build time — the same
/// standing risk [`super::environment`] documents for the four variable names —
/// so the string is a constant here, pinned by a test, and named in the module
/// docs above as the thing to look at when starts begin timing out.
pub const ANNOUNCEMENT: &str = "wt-media-agent local API listening on ";

/// How long between two looks at the buffer and the socket.
///
/// Not a config key: the smallest timeout a launch can carry is three orders of
/// magnitude above this, so there is no problem here for anyone to tune, and a
/// second knob would be a knob with nothing behind it.
const POLL: Duration = Duration::from_millis(25);

/// The port announced on a readiness line, if this line is one.
///
/// Split on the **last** colon, so an IPv6 host parses as well as a dotted quad:
/// the Agent prints whatever `agent.host` holds, and `::1` is an accepted
/// loopback spelling there (`config::is_loopback_host`).
pub fn announced(line: &str) -> Option<u16> {
    let address = line.strip_prefix(ANNOUNCEMENT)?;
    let (_, port) = address.rsplit_once(':')?;
    port.trim().parse().ok()
}

/// The first readiness line in the buffer, with the port it names.
///
/// The oldest rather than the newest: it is the moment the port opened, and an
/// Agent that announced twice did so by restarting, which is a different problem
/// from the one this looks for.
fn announcement(log: &SidecarLog) -> Option<(String, u16)> {
    drain::tail(log, drain::CAPACITY)
        .into_iter()
        .find_map(|line| announced(&line).map(|port| (line, port)))
}

/// What one look at `/healthz` found.
enum Answer {
    /// 2xx: listening, and this machine's credential is the one it demands.
    Ready(String),
    /// It answered and refused. Fatal.
    Refused(u16),
    /// Nothing came back. Worth another look until the deadline.
    Silent,
}

/// Ask `/healthz` once.
///
/// The status decides, not merely "did bytes arrive": `send()` succeeds for a
/// 401 exactly as it does for a 200, and the whole point of the second phase is
/// to distinguish them.
async fn probe(client: &LocalAgentClient) -> Answer {
    let response = match client.get("/healthz").send().await {
        Ok(response) => response,
        Err(_) => return Answer::Silent,
    };
    let status = response.status();
    if !status.is_success() {
        return Answer::Refused(status.as_u16());
    }
    match response.text().await {
        Ok(body) => Answer::Ready(body),
        Err(_) => Answer::Silent,
    }
}

/// The two readings a successful start produced.
#[derive(Debug)]
pub struct Ready {
    /// The Agent's announcement, verbatim.
    pub line: String,
    /// The body `/healthz` answered with.
    pub health: String,
}

/// Wait until the Agent is listening and answering, or give up at the deadline.
///
/// The deadline is `sidecar.start_timeout_ms` — the key that used to be
/// decoration. `Ok` carries both readings so the caller can record what it saw
/// rather than only that it waited.
pub async fn gate(
    config: &DesktopConfig,
    log: &SidecarLog,
    client: &LocalAgentClient,
) -> Result<Ready, String> {
    let timeout_ms = config.sidecar.start_timeout_ms;
    let expected = config.agent.port;
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let mut seen: Option<(String, u16)> = None;

    loop {
        if seen.is_none() {
            seen = announcement(log);
        }

        if let Some((line, port)) = &seen {
            if *port != expected {
                // Fatal on sight, not worth waiting out: no amount of time makes
                // the Agent answer on the port this machine is calling.
                return Err(format!(
                    "Local Agent 宣告在 {} 端口就绪，本机调用的却是 {} 端口；\
                     请检查 agent.port 与 sidecar 收到的 WT_MEDIA_LOCAL_API_PORT。",
                    port, expected
                ));
            }
            match probe(client).await {
                Answer::Ready(health) => {
                    return Ok(Ready {
                        line: line.clone(),
                        health,
                    })
                }
                Answer::Refused(status) => {
                    return Err(format!(
                        "Local Agent 已监听但拒绝了本机的凭据（HTTP {}）；\
                         两侧的运行时令牌不一致，本次启动中止。",
                        status
                    ))
                }
                Answer::Silent => {}
            }
        }

        if Instant::now() >= deadline {
            return Err(timed_out(timeout_ms, seen.as_ref().map(|(line, _)| line.as_str())));
        }
        tokio::time::sleep(POLL).await;
    }
}

/// Why the wait ended, told apart by whether the first phase ever fired.
///
/// The two texts differ because the two causes have nothing in common: "it never
/// said it was listening" points at a dead or wedged child, and "it said it was
/// listening and then said nothing" points at a socket that is open but not
/// answering requests. One message for both would send the reader to the wrong
/// half of the startup.
fn timed_out(ms: u64, seen: Option<&str>) -> String {
    match seen {
        Some(line) => format!(
            "Local Agent 在 {ms}ms 内报告了就绪（{line}），但健康检查始终没有应答；\
             端口已打开而未响应请求。"
        ),
        None => format!(
            "Local Agent 在 {ms}ms 内没有报告就绪；它没有打印监听行，可能未能启动或已退出。"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{load_with, Environment, PRODUCTION_TOML};
    use crate::token::RuntimeToken;
    use std::collections::BTreeMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// A config whose port is `port` and whose start timeout is `timeout_ms`.
    fn config(port: u16, timeout_ms: u64) -> DesktopConfig {
        let text = PRODUCTION_TOML
            .replace("port = 8765", &format!("port = {port}"))
            .replace(
                "start_timeout_ms = 15000",
                &format!("start_timeout_ms = {timeout_ms}"),
            );
        load_with(&BTreeMap::new(), &text, Environment::Production).expect("test config")
    }

    fn client_for(config: &DesktopConfig) -> LocalAgentClient {
        LocalAgentClient::new(config, RuntimeToken::generate())
    }

    /// Something that answers `/healthz` with `status`, on a kernel-chosen port.
    ///
    /// Its own stub rather than `commands::agent`'s: that one always answers 200,
    /// and the case this module has to get right is the status that is **not**
    /// 200. Port 0 so a test never fights the developer's running Agent for 8765.
    ///
    /// Answers every connection it is given, not just the first: the gate polls,
    /// and a stub that went deaf after one request would turn a working gate into
    /// a timeout.
    fn answering(status: u16, body: &'static str) -> u16 {
        let server = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let port = server.local_addr().expect("the bound address").port();
        std::thread::spawn(move || {
            for stream in server.incoming() {
                let Ok(mut socket) = stream else { continue };
                let mut request = [0u8; 2048];
                let _ = socket.read(&mut request);
                let response = format!(
                    "HTTP/1.1 {status} X\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(response.as_bytes());
            }
        });
        port
    }

    /// A health-answering stub plus the log the Agent's output would be in.
    fn harness(status: u16, body: &'static str, timeout_ms: u64) -> (DesktopConfig, SidecarLog) {
        let config = config(answering(status, body), timeout_ms);
        (config, SidecarLog::default())
    }

    /// Push the announcement the Agent really prints.
    fn announce(log: &SidecarLog, config: &DesktopConfig) -> String {
        let line = format!("{ANNOUNCEMENT}{}:{}", config.agent.host, config.agent.port);
        drain::push(log, line.clone());
        line
    }

    fn run(config: &DesktopConfig, log: &SidecarLog) -> Result<Ready, String> {
        tauri::async_runtime::block_on(gate(config, log, &client_for(config)))
    }

    /// The line the Agent prints is recognised, and its port read off it.
    ///
    /// Both spellings of a loopback host, because the port is the field after the
    /// last colon and `::1` has two more in front of it.
    #[test]
    fn the_announcement_is_recognised_and_its_port_read() {
        assert_eq!(
            announced("wt-media-agent local API listening on 127.0.0.1:8765"),
            Some(8765)
        );
        assert_eq!(
            announced("wt-media-agent local API listening on [::1]:8766"),
            Some(8766)
        );

        // Not the line at all: the Agent's other output, a near miss, and the
        // empty string. A prefix test that accepted any of these would let the
        // gate leave phase one on output that says nothing about the socket.
        for other in [
            "wt-media-agent local API stopped",
            "wt-media-agent local API listening",
            "wt-media-agent local API listening on 127.0.0.1",
            "wt-media-agent local API listening on 127.0.0.1:",
            "wt-media-agent local API listening on 127.0.0.1:notaport",
            " INFO wt-media-agent local API listening on 127.0.0.1:8765",
            "",
        ] {
            assert_eq!(announced(other), None, "{other:?} must not be the line");
        }
    }

    /// The whole gate, on the happy path: the timeout is never reached, and both
    /// readings come back.
    ///
    /// The timeout is deliberately tiny (200ms). If the gate waited it out and
    /// then reported success this test would still pass — which is why the
    /// failure cases below matter as much as this one.
    #[test]
    fn an_announcing_answering_agent_is_ready() {
        let (config, log) = harness(200, "{\"status\":\"ok\"}", 200);
        let line = announce(&log, &config);

        let ready = run(&config, &log).expect("it announces and answers");

        assert_eq!(ready.line, line, "the announcement comes back verbatim");
        assert_eq!(ready.health, "{\"status\":\"ok\"}");
    }

    /// **It never says it is listening.** The wait runs to the deadline and the
    /// message says so — not that it listened and went quiet.
    #[test]
    fn an_agent_that_never_announces_times_out() {
        let (config, log) = harness(200, "{}", 120);
        drain::push(&log, "starting".to_string());

        let error = run(&config, &log).expect_err("nothing ever announced");

        assert!(error.contains("120ms"), "the deadline is named: {error}");
        assert!(
            error.contains("没有报告就绪"),
            "the message must be the never-announced one: {error}"
        );
    }

    /// **It says it is listening and then nothing answers.** The other arm of the
    /// same wait, and the reason the two messages are separate.
    ///
    /// The announcement is real and the port it names is one nothing is listening
    /// on, so phase one fires and phase two never does.
    #[test]
    fn an_announcing_silent_agent_times_out_with_the_other_message() {
        let silent = {
            let probe = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
            probe.local_addr().expect("the bound address").port()
        };
        let config = config(silent, 120);
        let log = SidecarLog::default();
        drain::push(&log, format!("{ANNOUNCEMENT}127.0.0.1:{silent}"));

        let error = run(&config, &log).expect_err("nothing answers");

        assert!(
            error.contains("报告了就绪"),
            "it must say the announcement was seen: {error}"
        );
        assert!(
            !error.contains("没有报告就绪"),
            "and must not claim it never announced: {error}"
        );
        assert!(error.contains("端口已打开而未响应请求"), "{error}");
    }

    /// **The token does not match.** It answered 401, which is fatal on sight:
    /// the wait does not run to the deadline, and the message names the cause.
    ///
    /// This is the drift `super::environment` warns about — Desktop told the
    /// Agent one secret and calls with another. Timing is the evidence that it
    /// did not merely time out: the timeout is a full second and the assertion
    /// is that the call returned well inside it.
    #[test]
    fn a_refused_credential_fails_at_once_and_names_itself() {
        let (config, log) = harness(401, "{\"error\":\"unauthorized\"}", 1000);
        announce(&log, &config);

        let started = Instant::now();
        let error = run(&config, &log).expect_err("a 401 is not ready");
        let took = started.elapsed();

        assert!(error.contains("401"), "the status is named: {error}");
        assert!(
            error.contains("令牌不一致"),
            "the cause is named, not just the status: {error}"
        );
        assert!(
            took < Duration::from_millis(900),
            "a 401 must not be waited out: took {took:?}"
        );
    }

    /// **The Agent bound somewhere nobody is calling.** Fatal on sight, and the
    /// two ports are both named so the reader does not have to guess which is
    /// which.
    ///
    /// Nothing is listening on either port: if the wrong-port check were missing,
    /// this would run to the deadline and report a timeout instead.
    #[test]
    fn an_announcement_on_another_port_fails_at_once() {
        let (config, log) = harness(200, "{}", 1000);
        let bound = config.agent.port + 1;
        drain::push(&log, format!("{ANNOUNCEMENT}127.0.0.1:{bound}"));

        let started = Instant::now();
        let error = run(&config, &log).expect_err("the ports disagree");
        let took = started.elapsed();

        assert!(error.contains(&bound.to_string()), "{error}");
        assert!(error.contains(&config.agent.port.to_string()), "{error}");
        assert!(
            took < Duration::from_millis(900),
            "a port mismatch must not be waited out: took {took:?}"
        );
    }

    /// The wait is bounded by the **configured** timeout, both ways.
    ///
    /// Two different values, because one alone cannot tell "read from config"
    /// from "a constant that happens to match". The lower bound is loose on
    /// purpose — a loaded machine may overshoot — and the upper bound is what
    /// catches a gate that returns before waiting at all.
    #[test]
    fn the_wait_is_the_configured_one() {
        for timeout_ms in [100u64, 350] {
            let (config, log) = harness(200, "{}", timeout_ms);
            let started = Instant::now();
            let error = run(&config, &log).expect_err("nothing announced");
            let took = started.elapsed();

            assert!(error.contains(&timeout_ms.to_string()), "{error}");
            assert!(
                took >= Duration::from_millis(timeout_ms),
                "must not give up early at {timeout_ms}ms: took {took:?}"
            );
            assert!(
                took < Duration::from_millis(timeout_ms + 900),
                "must not run far past {timeout_ms}ms: took {took:?}"
            );
        }
    }

    /// It notices an announcement that arrives **after** the gate started.
    ///
    /// The gate is entered as soon as the child exists, and the announcement
    /// comes seconds later in a real launch. A gate that read the buffer once
    /// would pass the happy-path test above (which announces first) and fail on
    /// every real start.
    #[test]
    fn an_announcement_that_arrives_late_is_still_seen() {
        let (config, log) = harness(200, "{\"status\":\"ok\"}", 2000);
        let late = log.clone();
        let line = format!("{ANNOUNCEMENT}{}:{}", config.agent.host, config.agent.port);
        let pushed = line.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(120));
            drain::push(&late, pushed);
        });

        let ready = run(&config, &log).expect("it announces eventually");

        assert_eq!(ready.line, line);
    }

    /// **The real thing, on this machine — ignored by default.**
    ///
    /// The actual Agent (`../wt-media-agent`) on a scratch port, its stdout pumped
    /// into a `SidecarLog` exactly as `drain::follow` pumps it, and the same
    /// `gate` this crate ships. Nothing here is a double: the line comes from the
    /// Agent's own `print`, the socket is the Agent's, and `/healthz` is answered
    /// by the Agent's handler under the Agent's own auth check.
    ///
    /// That is the one thing the tests above cannot say. They prove the gate
    /// behaves correctly given a line; this proves that `ANNOUNCEMENT` is what the
    /// real Agent really prints, on a real stdout channel, once its port is open —
    /// the cross-repository half of the contract, which nothing checks at build
    /// time on either side.
    ///
    /// Ignored because it needs a sibling checkout and a `python3` on `PATH`,
    /// neither of which a plain `cargo test` may assume — the same reason
    /// CHG-058's `reveal_opens` is ignored. Run it with:
    ///
    /// ```text
    /// cargo test --manifest-path src-tauri/Cargo.toml -- --ignored real_agent
    /// ```
    #[test]
    #[ignore]
    fn the_gate_waits_for_a_real_agent_process() {
        use std::io::{BufRead, BufReader};
        use std::process::{Command, Stdio};

        let agent_repo =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../wt-media-agent");
        assert!(
            agent_repo.join("src/wt_media_agent").is_dir(),
            "需要 ../wt-media-agent 的检出：{}",
            agent_repo.display()
        );

        // A port the kernel hands out and takes back, so this never fights the
        // developer's own Agent for 8765 — the rule the whole suite follows.
        let port = {
            let probe = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
            probe.local_addr().expect("the bound address").port()
        };

        // The client owns the token, and the same value is what the child is told
        // — the ordering `commands::agent::start` uses, so a token that reached
        // only one side would fail here the way it would fail in a launch.
        let config = config(port, 8000);
        let client = LocalAgentClient::new(&config, RuntimeToken::generate());
        let token = client.token().expose().to_string();

        let mut child = Command::new("python3")
            .args(["-m", "wt_media_agent.local_api.server"])
            .current_dir(&agent_repo)
            .env("PYTHONPATH", "src")
            .env("WT_MEDIA_LOCAL_API_HOST", "127.0.0.1")
            .env("WT_MEDIA_LOCAL_API_PORT", port.to_string())
            .env("WT_MEDIA_AGENT_RUNTIME_TOKEN", token)
            .env(
                "WT_MEDIA_AGENT_DATA_DIR",
                std::env::temp_dir().join("wt-media-readiness-probe"),
            )
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("python3 must be runnable");

        let log = SidecarLog::default();
        let stdout = child.stdout.take().expect("a stdout pipe");
        let pusher = log.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                drain::push(&pusher, line.trim_end().to_string());
            }
        });

        let outcome = tauri::async_runtime::block_on(gate(&config, &log, &client));
        let _ = child.kill();
        let _ = child.wait();

        let ready = outcome.unwrap_or_else(|why| {
            panic!(
                "真机闸门失败：{why}\n缓冲：{:?}",
                drain::tail(&log, 20)
            )
        });
        assert_eq!(
            ready.line,
            format!("{ANNOUNCEMENT}127.0.0.1:{port}"),
            "the real Agent's line must be the one this module parses"
        );
        assert!(
            ready.health.contains("\"status\""),
            "the health body must be the Agent's own: {}",
            ready.health
        );
    }
}
