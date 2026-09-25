//! Checking the bundled sidecar against the record the package carries.
//!
//! The release pipeline has recorded a SHA-256 for the embedded sidecar for a
//! while (`build_desktop_sidecar.py` at build time, `package-release-macos.sh`
//! at packaging time, and `SHA256SUMS` beside them), and **nothing read any of
//! it**. A package whose sidecar was truncated, swapped, or left over from
//! another build produced the same reading as an intact one: a working start, or
//! a `dlopen` failure deep inside PyInstaller's bootloader, with no way to tell a
//! broken download from a broken build. This module is that reader.
//!
//! Three facts from the packaging scripts shape it:
//!
//! 1. **The digest cannot be computed at build time.** `repair-macos-signing.sh`
//!    re-signs the sidecar ad-hoc (`codesign --force --sign -`) because Tauri's
//!    hardened-runtime signature makes macOS refuse the linker-signed
//!    `libpython3.14.dylib` nested inside it, and signing **rewrites the file**
//!    (measured: `e37653fa…` → `10fdf5ca…`). So the build manifest's digest and
//!    the packaged file's digest legitimately differ, and an equality assertion
//!    between them would fail every release. The record this module reads is the
//!    one written *after* the last write to the sidecar.
//! 2. **The record therefore lives beside the signed app**, not in the build
//!    output: `Contents/Resources/sidecar-manifest.json`, written between the
//!    sidecar's signature and the outer app's (measured: signing the outer app
//!    with `--options runtime` afterwards leaves the sidecar's digest unchanged
//!    and still passes `codesign --verify --deep --strict`, so the seal covers
//!    the record rather than invalidating it).
//! 3. **The file that is verified has to be the file that is started.** Verifying
//!    a path and then starting a *name* would let two resolutions disagree; see
//!    `resolve_sidecar` and `sidecar::spawn_verified`.
//!
//! What this is **not**: a security boundary against someone who can write the
//! app bundle. The app is ad-hoc signed (`tauri.conf.json`, `signingIdentity:
//! "-"`), so there is no trust anchor to root an authenticity claim in, and
//! whoever can replace the sidecar can replace the app that checks it. What it
//! does catch is the failure this pipeline actually has: a distribution whose
//! sidecar does not match what the package says was shipped. Refusing to start
//! is the right answer there — a mismatched Agent is not a degraded app, it is an
//! app whose every later call is answered by something nobody vouched for.
//!
//! `Location::check` is the whole decision, and it is a small one:
//!
//! | sidecar | manifest | running from a bundle | answer |
//! |---|---|---|---|
//! | absent | — | — | [`Checked::Absent`] — there is nothing to check, and the start fails at the spawn as before |
//! | present | matches | — | [`Checked::Matched`] |
//! | present | missing | yes | **refused**: an incomplete package, and deleting the record must not be a way past it |
//! | present | missing | no | [`Checked::Unrecorded`] — a developer's tree, see below |
//! | present | present, wrong | — | **refused**, whatever the shape of the disagreement |
//!
//! The fourth row is the one deliberate exemption, and it exists because
//! `tauri-build`'s build script copies `externalBin` into the cargo target
//! directory (`tauri-build-2.6.3/src/lib.rs:546`), so a developer who has built
//! the sidecar gets `target/debug/wt-media-agent` — a *copy* of a build input,
//! with no record anywhere near it. A package that ships a sidecar always has a
//! record (rule 2 above), so "in a bundle and unrecorded" is a fact about a
//! distribution and "outside a bundle and unrecorded" is a fact about a working
//! tree. The exemption is therefore not a hole to climb through: to reach it you
//! have to take the app out of the `.app` it was installed as, at which point you
//! are no longer attacking the package's claim about itself.

use tauri::{AppHandle, Manager, Runtime};

/// The stable runtime name of the bundled sidecar.
///
/// Tauri resolves `externalBin: ["binaries/wt-media-agent"]` from the
/// target-qualified *build* filename and installs it under this name
/// (`package-release-macos.sh` relies on the same string). The manifest has to
/// name the same file, or it is describing something else.
pub const SIDECAR_NAME: &str = "wt-media-agent";

/// The record's filename, in the bundle's resource directory.
pub const MANIFEST_NAME: &str = "sidecar-manifest.json";

/// The `component` a manifest for this sidecar must name.
const COMPONENT: &str = "wt-media-agent";

/// What `sidecar-manifest.json` says about the sidecar beside it.
///
/// All five keys are required, and that is not stricter than it needs to be: a
/// record that does not say *which* component and *which* file it describes
/// cannot be checked against a file at all, and the packaging scripts write all
/// five. `version` and `target` are read here because T-06 needs them for the
/// Desktop↔sidecar version comparison; the digest is the only one this module
/// uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub component: String,
    pub version: String,
    pub target: String,
    pub filename: String,
    pub sha256: String,
}

/// The answer to "is this sidecar the one the package says it is".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Checked {
    /// The record is present and the file matches it.
    Matched(Manifest),
    /// There is no sidecar here at all, so there is nothing to check. The start
    /// then fails at the spawn, exactly as it did before this module existed.
    Absent,
    /// A sidecar is present with no record beside it, and this launch is not
    /// coming out of a bundle — a build tree. Says so in the log rather than
    /// passing in silence, because the sidecar it is about to run is unchecked.
    Unrecorded,
}

/// Where this launch's sidecar and its record are, and whether this launch came
/// out of a package.
///
/// All three are computed together, once, by [`Location::of`], and then used for
/// both the check and the spawn. Resolving twice is how a verified file and a
/// started file become two different files.
#[derive(Clone, Debug)]
pub struct Location {
    sidecar: std::path::PathBuf,
    manifest: std::path::PathBuf,
    bundled: bool,
}

impl Location {
    /// The production constructor — the only one outside tests.
    ///
    /// The sidecar is resolved from `current_exe()` rather than asked of
    /// `tauri_plugin_shell`, because the plugin's `relative_command_path` is
    /// private (`tauri-plugin-shell-2.3.5/src/process/mod.rs:120`) while its
    /// algorithm is three lines; `resolve_sidecar` is that algorithm, and the
    /// plugin's `sidecar(name)` is literally `command(relative_command_path(name))`
    /// (`:181`), so spawning the resolved path is the same spawn with the path
    /// kept in hand.
    pub fn of<R: Runtime>(app: &AppHandle<R>) -> Result<Self, String> {
        let exe = std::env::current_exe().map_err(|e| format!("无法确定本程序的路径：{e}"))?;
        let resources = app
            .path()
            .resource_dir()
            .map_err(|e| format!("无法确定安装包的资源目录：{e}"))?;
        Ok(Self {
            sidecar: resolve_sidecar(&exe),
            manifest: resources.join(MANIFEST_NAME),
            bundled: inside_a_bundle(&exe),
        })
    }

    /// A location of the caller's choosing, for tests.
    ///
    /// Test-only on purpose: there is no production reason to point the check at
    /// somewhere other than where the app's own sidecar is, and a constructor a
    /// release build could reach is a constructor a release build can get wrong.
    #[cfg(test)]
    pub fn at(sidecar: std::path::PathBuf, manifest: std::path::PathBuf, bundled: bool) -> Self {
        Self {
            sidecar,
            manifest,
            bundled,
        }
    }

    pub fn sidecar(&self) -> &std::path::Path {
        &self.sidecar
    }

    /// Whether this launch came out of a bundle, as [`Location::of`] decided it.
    ///
    /// Test-only, like [`Location::at`]: the flag is private because only
    /// [`Location::check`] may act on it, and reading it from a release build
    /// would be the first step towards a second answer to "is this a package".
    /// It exists so a test can pin that `of` gets it from the executable's path —
    /// otherwise the two halves of the rule ("the flag decides refusal" and "the
    /// flag means a `.app`") could each be tested while nothing tied them
    /// together, which is exactly the shape of a hole.
    #[cfg(test)]
    pub fn bundled(&self) -> bool {
        self.bundled
    }

    /// Where the record was looked for. Test-only, like [`Location::bundled`]:
    /// nothing in a release build needs to name it, and
    /// `the_bundled_sidecar_…` below is the only reader.
    #[cfg(test)]
    pub fn manifest(&self) -> &std::path::Path {
        &self.manifest
    }

    /// The decision table in this module's header, and nothing else.
    ///
    /// Every path out of here is one of three: checked and matching, nothing to
    /// check, or a refusal that carries the reason and the path it is about. A
    /// refusal is the caller's to report — `sidecar::spawn_verified` turns it
    /// into an `Err` and does **not** fall through to the Python path: a
    /// mismatched sidecar means this package is not the one that was shipped, and
    /// quietly starting a different Agent instead is the one answer that hides it.
    pub fn check(&self) -> Result<Checked, String> {
        if !self.sidecar.is_file() {
            return Ok(Checked::Absent);
        }
        if !self.manifest.is_file() {
            return if self.bundled {
                Err(format!(
                    "随应用的 Local Agent 校验失败：包内缺少记录文件 {}。\
                     这个安装包不完整，请重新安装完整的 WT Media 安装包。",
                    self.manifest.display()
                ))
            } else {
                Ok(Checked::Unrecorded)
            };
        }
        let manifest = read(&self.manifest)?;
        let actual = digest(&self.sidecar)?;
        if manifest.sha256 != actual {
            return Err(format!(
                "随应用的 Local Agent 校验失败：{} 的 SHA-256 与包内记录不一致\
                 （记录 {}，实际 {}）。这个文件已被替换或损坏，\
                 请重新安装完整的 WT Media 安装包。",
                self.sidecar.display(),
                manifest.sha256,
                actual
            ));
        }
        Ok(Checked::Matched(manifest))
    }
}

/// Say in the log what the check found, so a start can be read afterwards.
///
/// Three outcomes, two records: a matching record is stated with the version and
/// digest it matched (that is the reading a packaged start is diagnosed from),
/// and an unrecorded sidecar is a warning, because Desktop is about to run a file
/// nothing compared against anything. `Absent` is silent — every developer's tree
/// and every `cargo test` run is in that state, and the failure that follows a
/// start with no sidecar already names itself.
pub fn note(checked: &Checked, location: &Location) {
    match checked {
        Checked::Matched(manifest) => tracing::info!(
            target: "agent.supervisor",
            sha256 = %manifest.sha256,
            version = %manifest.version,
            native_target = %manifest.target,
            "随应用的 Local Agent 校验通过"
        ),
        Checked::Unrecorded => tracing::warn!(
            target: "agent.supervisor",
            path = %location.sidecar().display(),
            "随应用的 Local Agent 没有包内记录，本次启动未校验"
        ),
        Checked::Absent => {}
    }
}

/// The path the sidecar is installed at, from the running executable.
///
/// **The same algorithm as `tauri_plugin_shell`'s `relative_command_path`**
/// (`tauri-plugin-shell-2.3.5/src/process/mod.rs:120-152`), including the `deps`
/// hop: under `cargo test` the executable is `target/<profile>/deps/<crate>-<hash>`
/// while the sidecar Tauri's build script copies sits one level up, in
/// `target/<profile>/` (`tauri-build-2.6.3/src/lib.rs:546`). Without the hop,
/// every dev-tree launch would look for a sidecar that is not there. The Windows
/// `.exe` handling below is copied from the same function and is **unverified** —
/// no Windows package has been built here (Q-03).
pub fn resolve_sidecar(exe: &std::path::Path) -> std::path::PathBuf {
    let dir = exe.parent().unwrap_or(exe);
    let dir = if dir.ends_with("deps") {
        dir.parent().unwrap_or(dir)
    } else {
        dir
    };
    #[cfg(not(windows))]
    let path = dir.join(SIDECAR_NAME);
    #[cfg(windows)]
    let path = {
        let mut path = dir.join(SIDECAR_NAME);
        if !path.extension().is_some_and(|ext| ext == "exe") {
            // `with_extension` would eat a dot in the name; the plugin pushes.
            path.as_mut_os_string().push(".exe");
        }
        path
    };
    path
}

/// Whether this launch came out of a macOS `.app`. See the fourth row of the
/// table above for what depends on the answer.
pub fn inside_a_bundle(exe: &std::path::Path) -> bool {
    exe.ancestors()
        .any(|step| step.file_name() == Some(std::ffi::OsStr::new("Contents")))
}

/// The SHA-256 of a whole file, as lowercase hex.
///
/// Streamed rather than read into memory: the sidecar is a 10 MB PyInstaller
/// bundle today and there is no reason for the check to be the thing that makes
/// a larger one fail to start.
pub fn digest(path: &std::path::Path) -> Result<String, String> {
    use sha2::Digest as _;
    let mut file =
        std::fs::File::open(path).map_err(|e| format!("无法打开 {}：{e}", path.display()))?;
    let mut hasher = sha2::Sha256::new();
    std::io::copy(&mut file, &mut hasher)
        .map_err(|e| format!("读取 {} 时失败：{e}", path.display()))?;
    Ok(hex::encode(hasher.finalize()))
}

/// Read a manifest, requiring every key it has to have.
///
/// A field that is there but is not a string is the same refusal as a field that
/// is missing: both mean the record cannot be compared against a file, and a
/// half-readable record is not a weaker check than a missing one — it is the
/// opposite, since it would let a package keep the *appearance* of being
/// recorded.
fn read(path: &std::path::Path) -> Result<Manifest, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("无法读取校验文件 {}：{e}", path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("校验文件 {} 不是可解析的 JSON：{e}", path.display()))?;
    let field = |key: &str| {
        value
            .get(key)
            .and_then(|held| held.as_str())
            .map(str::to_owned)
            .ok_or_else(|| {
                format!(
                    "校验文件 {} 没有可用的 {} 字段，无法与 sidecar 比对。",
                    path.display(),
                    key
                )
            })
    };
    let manifest = Manifest {
        component: field("component")?,
        version: field("version")?,
        target: field("target")?,
        filename: field("filename")?,
        sha256: field("sha256")?,
    };
    if manifest.component != COMPONENT {
        return Err(format!(
            "校验文件 {} 记录的是 {}，不是 {}。",
            path.display(),
            manifest.component,
            COMPONENT
        ));
    }
    if manifest.filename != SIDECAR_NAME {
        return Err(format!(
            "校验文件 {} 记录的文件名是 {}，而随应用安装的是 {}。",
            path.display(),
            manifest.filename,
            SIDECAR_NAME
        ));
    }
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use std::path::{Path, PathBuf};

    /// A scratch directory of this test's own, removed when it is dropped.
    ///
    /// The removal matters here more than in most tests: `Location::check` reads
    /// whatever sidecar a caller names, and a leftover `wt-media-agent` under a
    /// target directory is a file some later run would execute.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("wt-media-integrity-{name}"));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("scratch directory");
            Self(dir)
        }

        fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, bytes).expect("write");
            path
        }

        /// A sidecar this test can point at, holding the given bytes.
        fn sidecar(&self, bytes: &[u8]) -> PathBuf {
            self.file(SIDECAR_NAME, bytes)
        }

        /// A record for `sidecar`, describing it correctly unless `sha256` says
        /// otherwise, so a test can produce a disagreement by changing one value.
        fn manifest(&self, sidecar: &Path, sha256: Option<&str>) -> PathBuf {
            let recorded = match sha256 {
                Some(given) => given.to_string(),
                None => digest(sidecar).expect("digest"),
            };
            self.file(
                MANIFEST_NAME,
                format!(
                    "{{\"component\":\"{COMPONENT}\",\"version\":\"0.2.5\",\
                     \"target\":\"aarch64-apple-darwin\",\"filename\":\"{SIDECAR_NAME}\",\
                     \"sha256\":\"{recorded}\"}}"
                )
                .as_bytes(),
            )
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The judgement this task exists for, in both directions and in one test.
    ///
    /// One byte appended is the smallest disagreement there is, and it is the one
    /// the recorded digest has no way to absorb: if a single changed byte passed,
    /// the check would be comparing something other than the file's content.
    /// The intact arm comes first so the tampered arm cannot pass by refusing
    /// everything.
    ///
    /// The file is deliberately **larger than any plausible prefix**: the first
    /// version of this test used 34 bytes, and a `digest` mutated to hash only the
    /// first 128 bytes survived it — the mutation was equivalent for a file that
    /// short, so the test could not tell a whole-file digest from a partial one.
    #[test]
    fn a_sidecar_that_matches_its_record_passes_and_one_changed_byte_does_not() {
        let scratch = Scratch::new("one-byte");
        let mut bytes = vec![b'p'; 4096];
        bytes.push(b'\n');
        let sidecar = scratch.sidecar(&bytes);
        let manifest = scratch.manifest(&sidecar, None);

        let intact = Location::at(sidecar.clone(), manifest.clone(), true);
        let matched = intact.check().expect("an intact sidecar passes");
        let Checked::Matched(record) = matched else {
            panic!("expected a match, got {matched:?}");
        };
        assert_eq!(record.filename, SIDECAR_NAME);
        assert_eq!(record.sha256, digest(&sidecar).expect("digest"));

        std::fs::OpenOptions::new()
            .append(true)
            .open(&sidecar)
            .expect("append")
            .write_all(b"\0")
            .expect("write the tampered byte");
        let tampered = Location::at(sidecar.clone(), manifest, true);
        let refusal = tampered.check().expect_err("one byte is a disagreement");
        assert!(
            refusal.contains("SHA-256 与包内记录不一致"),
            "the refusal has to say what is wrong: {refusal}"
        );
        assert!(
            refusal.contains(&sidecar.display().to_string()),
            "and which file it is about: {refusal}"
        );
    }

    /// The digest is a reading of the file, not of this module's own arithmetic.
    ///
    /// `diagnostic.rs` made the same move for the same reason (its own test says
    /// so): a digest checked against a digest computed here would be
    /// self-consistent and could still be wrong. This asks the machine instead —
    /// `shasum` (`sha256sum` on Linux), whichever is present.
    #[test]
    fn the_digest_is_the_one_the_machine_computes() {
        use std::process::Command;
        let scratch = Scratch::new("digest");
        let sidecar = scratch.sidecar(b"not a real sidecar, but a real file\n");

        let ours = digest(&sidecar).expect("digest");
        let mut theirs = None;
        for (program, args) in [("shasum", ["-a", "256"]), ("sha256sum", ["", ""])] {
            if let Ok(output) = Command::new(program).args(args).arg(&sidecar).output() {
                if output.status.success() {
                    theirs = Some(
                        String::from_utf8_lossy(&output.stdout)
                            .split_whitespace()
                            .next()
                            .unwrap_or_default()
                            .to_string(),
                    );
                    break;
                }
            }
        }
        let theirs = theirs.expect("neither `shasum` nor `sha256sum` is on this machine");
        assert_eq!(ours, theirs, "the check and the machine must agree");
        assert_eq!(ours.len(), 64, "a sha256 in hex is 64 characters");
    }

    /// A record that is missing, torn, or about another component is refused —
    /// and so is one about another file.
    ///
    /// Each case names the **reason** it must produce, not merely that something
    /// was refused. That is not decoration: a version of `read` that fell back to
    /// an empty string for a missing key still refused (the empty digest does not
    /// match the file), so an assertion on "refused" alone could not tell the two
    /// apart. "报出原因" is half of what T-04 asks for, so it is what is asserted.
    #[test]
    fn a_record_that_cannot_be_compared_is_refused() {
        let scratch = Scratch::new("malformed");
        let sidecar = scratch.sidecar(b"bytes");
        let good = std::fs::read_to_string(scratch.manifest(&sidecar, None)).expect("read");

        let cases: [(&str, String, &str); 6] = [
            ("not JSON at all", "{".to_string(), "不是可解析的 JSON"),
            (
                "no sha256",
                good.replace(
                    &format!(",\"sha256\":\"{}\"", digest(&sidecar).expect("digest")),
                    "",
                ),
                "sha256 字段",
            ),
            (
                // Valid JSON, wrong type: the record exists but cannot be read,
                // which is a different refusal from a record that is not JSON.
                "sha256 is not a string",
                good.replace(&format!("\"{}\"", digest(&sidecar).expect("digest")), "7"),
                "sha256 字段",
            ),
            (
                "another component",
                good.replace(COMPONENT, "wt-media-cloud"),
                "不是",
            ),
            (
                // Only the `filename` key: the component is the same string, so a
                // bare replacement would trip the component rule first and this
                // case would be asserting about the wrong refusal.
                "another filename",
                good.replace(
                    &format!("\"filename\":\"{SIDECAR_NAME}\""),
                    "\"filename\":\"wt-media-agent-arm64\"",
                ),
                "文件名是",
            ),
            ("empty", String::new(), "不是可解析的 JSON"),
        ];
        for (what, text, expected) in cases {
            let path = scratch.file("torn.json", text.as_bytes());
            let location = Location::at(sidecar.clone(), path, true);
            let refusal = location
                .check()
                .expect_err(&format!("{what} has to be refused"));
            assert!(
                refusal.contains("校验") && refusal.contains(expected),
                "the refusal has to say {expected:?} for {what}: {refusal}"
            );
        }

        // The control for the six above: the same bytes that produced them pass.
        let matched = Location::at(sidecar.clone(), scratch.manifest(&sidecar, None), true)
            .check()
            .expect("the untorn record passes");
        assert!(matches!(matched, Checked::Matched(_)));
    }

    /// Deleting the record must not be a way past the check — inside a bundle.
    ///
    /// Both arms, because the exemption is only defensible if it is exactly as
    /// wide as it says: a build tree is tolerated, a package is not.
    #[test]
    fn a_missing_record_is_refused_in_a_package_and_tolerated_in_a_build_tree() {
        let scratch = Scratch::new("missing");
        let sidecar = scratch.sidecar(b"bytes");
        let absent = scratch.0.join(MANIFEST_NAME);
        assert!(!absent.exists());

        let packaged = Location::at(sidecar.clone(), absent.clone(), true);
        let refusal = packaged
            .check()
            .expect_err("a packaged sidecar without its record is not shippable");
        assert!(
            refusal.contains(&absent.display().to_string()),
            "the refusal names the file it looked for: {refusal}"
        );

        let tree = Location::at(sidecar.clone(), absent, false);
        assert_eq!(
            tree.check().expect("a build tree is tolerated"),
            Checked::Unrecorded
        );

        // And the control for both: with the record present, the packaged arm
        // stops refusing, so the refusal above is about the record and not about
        // the sidecar or the `bundled` flag.
        let recorded = Location::at(sidecar.clone(), scratch.manifest(&sidecar, None), true);
        assert!(matches!(
            recorded.check().expect("recorded"),
            Checked::Matched(_)
        ));
    }

    /// No sidecar, nothing to check — the state every `cargo test` run is in.
    #[test]
    fn no_sidecar_is_not_a_failure() {
        let scratch = Scratch::new("absent");
        let location = Location::at(
            scratch.0.join(SIDECAR_NAME),
            scratch.0.join(MANIFEST_NAME),
            true,
        );
        assert_eq!(
            location.check().expect("nothing to check is not an error"),
            Checked::Absent
        );
    }

    /// The path formula, including the `deps` hop — the one part of this module
    /// that has to agree with another crate's private function.
    #[test]
    fn the_sidecar_is_resolved_beside_the_app_and_out_of_deps() {
        let bundle = Path::new("/Applications/WT Media.app/Contents/MacOS/wt-media-desktop");
        assert_eq!(
            resolve_sidecar(bundle),
            Path::new("/Applications/WT Media.app/Contents/MacOS").join(SIDECAR_NAME)
        );

        let under_test = Path::new("/build/target/debug/deps/wt_media_desktop_shell-1a2b3c");
        assert_eq!(
            resolve_sidecar(under_test),
            Path::new("/build/target/debug").join(SIDECAR_NAME)
        );

        // A directory that merely *ends* with the four letters is not `deps`.
        let close = Path::new("/build/target/deps-2/wt-media-desktop");
        assert_eq!(
            resolve_sidecar(close),
            Path::new("/build/target/deps-2").join(SIDECAR_NAME)
        );
    }

    /// Whether the launch came out of a package, told apart from a build tree.
    #[test]
    fn only_a_launch_from_inside_a_bundle_counts_as_one() {
        assert!(inside_a_bundle(Path::new(
            "/Applications/WT Media.app/Contents/MacOS/wt-media-desktop"
        )));
        assert!(!inside_a_bundle(Path::new(
            "/build/target/debug/wt-media-desktop"
        )));
        assert!(!inside_a_bundle(Path::new(
            "/build/target/debug/deps/wt_media_desktop_shell-1a2b3c"
        )));
        assert!(!inside_a_bundle(Path::new("wt-media-desktop")));
    }
}
