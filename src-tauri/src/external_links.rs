//! Where a link the page opens in a new window actually lands.
//!
//! The packaged app is a Tauri v2 webview, and a webview is not a browser. The
//! front end's `window.open` — and every `<a target="_blank">`, which takes the
//! same path — does not make a window by itself. It raises a *new window
//! request*, which this shell either answers or drops, and with no handler
//! installed it is dropped: `window.open` returns `null` and the click goes
//! nowhere. That is what an operator hit — 素材详情的「打开云端视频」reported
//! 「浏览器拦截了新标签页」, and the same silence swallowed 「平台原视频」 and
//! 「作者主页」, which are plain anchors.
//!
//! So a new window request is answered by handing the address to the OS and
//! denying the in-app window. That is where a link like this belongs: a prepared
//! material's video is a 172 MB stream on the object store, and the browser a
//! person already has is what can play it. The app never grows a second window,
//! and the page does not have to know which kind of host it is running in.
//!
//! ## Why this is not a command
//!
//! The page asks to **navigate**; it does not ask this process to open
//! something. `commands::reveal`'s header forbids the other shape — a command
//! that takes a path from the page is a general 「打开任意位置」primitive — and a
//! command taking a URL would be the same primitive with a different noun. Here
//! the address is one the page was already navigating to, and the only decision
//! the shell makes is which window it lands in.
//!
//! ## Why the main window is built here
//!
//! `on_new_window` exists only on the webview builder (tauri 2.11.5
//! `src/webview/mod.rs:585`, taking `self` by value) and `tauri::Builder` has no
//! hook that reaches a webview once it exists. A window declared in
//! `tauri.conf.json` is built by `tauri::app::setup` *before* the app's own setup
//! closure runs (`app.rs:2522-2536`), so nothing can be attached to it in time.
//! `"create": false` is what lifts it out of that loop (`app.rs:2524`), and
//! [`install`] rebuilds it with `WebviewWindowBuilder::from_config` — the very
//! call that loop makes — so title, size and label all still come from one place.
//!
//! ## The line that cannot be tested here
//!
//! [`install`] hands an address to the platform's opener through the `open`
//! crate. Whether the browser then comes to the front is not something a test can
//! assert, and a test that spawned it would open a tab on whatever machine ran
//! the suite. So the tested half is the decision — which addresses are handed
//! over and which are refused — and the spawn itself is checked by hand on this
//! machine, the same split `commands::reveal` registers.
//!
//! Nothing here records the address. A prepared material's video URL is a signed
//! grant: it is a credential, and `logging::targets` would file it under a target
//! whose whole point is being readable later.

use tauri::{
    utils::config::WindowConfig,
    webview::{NewWindowResponse, WebviewWindow, WebviewWindowBuilder},
    AppHandle, Url,
};

/// The window `capabilities/default.json` is scoped to.
///
/// `"main"` is also the label Tauri's own window-creation loop would have used
/// for the single declaration in `tauri.conf.json`; naming it here keeps the
/// capability's scope and the window this module builds the same string.
const MAIN_WINDOW: &str = "main";

/// Whether an address is one the system browser may be asked to open.
///
/// `http`/`https` only. A new window request carries whatever scheme the page
/// wrote, and the schemes that are not web addresses are the ones that do damage
/// when an opener resolves them: `file:` opens a local file, `javascript:` and
/// `data:` run in the opener's context or hand it attacker-authored bytes, and
/// any other scheme with a registered handler launches that application. This app
/// opens a new window only for links it renders from the platform and for a
/// signed object address, so refusing the rest costs nothing.
///
/// **No host test, and that is a measurement rather than an omission.** `http`
/// and `https` are special schemes, so the parser always produces a host for
/// them — it rewrites `http:foo` to `http://foo/`, `https:/x` to `http://x/`, and
/// rejects the hostless `http://` outright ([`the_parser_leaves_no_hostless_http_url`]
/// pins that). A `host().is_some()` clause would therefore be a branch no input
/// can reach; the arm it would guard is upstream's, not ours.
fn hands_to_the_system_browser(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
}

/// Build the main window with the new window handler attached.
///
/// Called from the app's setup closure, so the window exists before `run` starts
/// serving it — which is where a `"create": false` declaration would otherwise
/// leave a launch with no window at all.
pub fn install(app: &AppHandle) -> Result<WebviewWindow, Box<dyn std::error::Error>> {
    let config = window_config(app)?;
    let window = WebviewWindowBuilder::from_config(app, &config)?
        .on_new_window(|url, _features| {
            if hands_to_the_system_browser(&url) {
                // Dropped rather than reported: the request has already been
                // answered by the `Deny` below, so there is no channel left to
                // tell the page that the opener refused. A machine with no
                // browser registered is the only way this fails.
                let _ = open::that(url.as_str());
            }
            NewWindowResponse::Deny
        })
        .build()?;
    Ok(window)
}

/// The declaration this module builds the window from.
///
/// Refused rather than defaulted: a config with no such window would otherwise
/// produce a launch that runs with no window, which reads as a hung app instead
/// of as the broken build it is.
fn window_config(app: &AppHandle) -> Result<WindowConfig, Box<dyn std::error::Error>> {
    app.config()
        .app
        .windows
        .iter()
        .find(|window| window.label == MAIN_WINDOW)
        .cloned()
        .ok_or_else(|| {
            format!("tauri.conf.json declares no window labelled `{MAIN_WINDOW}`").into()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(url: &str) -> Url {
        Url::parse(url).unwrap_or_else(|error| panic!("{url} did not parse: {error}"))
    }

    #[test]
    fn the_signed_object_address_the_drawer_hands_over_is_handed_on() {
        // The one this module exists for, in the shape the API returns it:
        // https, host, path-style, query carrying the signature.
        let url = parse(
            "https://s3.oss.longyanyue.cn/data/dev/materials/30/2afa51e6.mp4\
             ?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Signature=stub",
        );
        assert!(hands_to_the_system_browser(&url));
        // The control arm: a platform link, http rather than https, is also a
        // web address.
        assert!(hands_to_the_system_browser(&parse(
            "http://127.0.0.1:18080/api/v1/health"
        )));
    }

    #[test]
    fn a_local_file_is_not_a_link_to_open_in_a_browser() {
        assert!(!hands_to_the_system_browser(&parse("file:///etc/passwd")));
    }

    #[test]
    fn a_script_or_a_data_url_is_not_handed_to_an_opener() {
        assert!(!hands_to_the_system_browser(&parse("javascript:alert(1)")));
        assert!(!hands_to_the_system_browser(&parse(
            "data:text/html,<script>alert(1)</script>"
        )));
    }

    #[test]
    fn a_scheme_with_its_own_application_is_not_handed_over() {
        // `mailto:` opens a mail client and `smb:` mounts a share. Neither is a
        // page the browser is the right destination for.
        assert!(!hands_to_the_system_browser(&parse(
            "mailto:someone@example.com"
        )));
        assert!(!hands_to_the_system_browser(&parse("smb://server/share")));
    }

    /// Why [`hands_to_the_system_browser`] has no host arm, asserted upstream.
    ///
    /// Measured on the `url` version in this lockfile: every spelling that could
    /// have produced an addressless `http`/`https` URL is normalised into one
    /// that has a host, or refused. So the clause is unreachable rather than
    /// merely unwritten — and if a future `url` ever stops normalising,
    /// `http:foo` and `https:/x` here are what says so.
    #[test]
    fn the_parser_leaves_no_hostless_http_url() {
        for raw in ["http:foo", "http:///x", "https:/x"] {
            let url = parse(raw);
            assert!(
                url.host_str().is_some_and(|host| !host.is_empty()),
                "{raw} parsed without a host"
            );
        }
        assert!(
            Url::parse("http://").is_err(),
            "an empty host is accepted now"
        );
    }
}
