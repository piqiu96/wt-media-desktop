"""Packaging identity and resource routing regression, without building apps."""
import importlib.util
import json
import shutil
import subprocess
import tempfile
import unittest
from unittest import mock
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location(
    "desktop_environment", ROOT / "scripts/build-desktop-environment.py"
)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class DesktopEnvironmentTests(unittest.TestCase):
    def test_packaged_config_and_identity_match_while_cloud_target_stays_prod(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / "src-tauri/resources").mkdir(parents=True)
            for name in ["tauri.conf.json", "resources/desktop.production.toml"]:
                (root / "src-tauri" / name).write_bytes((ROOT / "src-tauri" / name).read_bytes())
            for env, name in [("dev", "起飞 Dev"), ("test", "起飞 Test"), ("prod", "起飞")]:
                overlay = json.loads(module.prepare(root, env, "debug" if env == "dev" else "release").read_text())
                self.assertEqual(overlay["productName"], name)
                self.assertEqual(overlay["identifier"], "com.wtmedia.desktop" + ("" if env == "prod" else "." + env))
                resources = overlay["bundle"]["resources"]
                self.assertIsNone(resources["resources/desktop.production.toml"])
                runtime = next(Path(path) for path in resources if resources[path] == "resources/desktop.production.toml")
                config = module.tomllib.loads(runtime.read_text())
                self.assertEqual(config["environment"], env)
                self.assertEqual(config["cloud"]["cloud_env"], "prod")

    def test_packaging_files_are_utf8_under_a_legacy_windows_locale(self):
        read_text, write_text = Path.read_text, Path.write_text

        def legacy_read(path, *args, **kwargs):
            kwargs.setdefault("encoding", "cp1252")
            return read_text(path, *args, **kwargs)

        def legacy_write(path, data, *args, **kwargs):
            kwargs.setdefault("encoding", "cp1252")
            return write_text(path, data, *args, **kwargs)

        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / "src-tauri/resources").mkdir(parents=True)
            for name in ["tauri.conf.json", "resources/desktop.production.toml"]:
                (root / "src-tauri" / name).write_bytes((ROOT / "src-tauri" / name).read_bytes())
            with mock.patch.object(Path, "read_text", legacy_read), mock.patch.object(Path, "write_text", legacy_write):
                for env, name in [("dev", "起飞 Dev"), ("test", "起飞 Test"), ("prod", "起飞")]:
                    overlay = module.prepare(root, env, "debug" if env == "dev" else "release")
                    self.assertEqual(json.loads(overlay.read_bytes().decode("utf-8"))["productName"], name)
                    config = module.tomllib.loads((overlay.parent / "desktop.production.toml").read_bytes().decode("utf-8"))
                    self.assertEqual(config["environment"], env)

    def test_release_cannot_accidentally_package_development_identity(self):
        with self.assertRaises(ValueError):
            module.prepare(ROOT, "dev", "release")

    def test_local_formal_entry_pins_release_gate(self):
        self.assertIn("export WT_MEDIA_DESKTOP_RELEASE_CHANNEL=stable", (ROOT / "scripts/build-release-macos.sh").read_text())

    def test_finalization_follows_cargo_target_directory_and_explicit_target(self):
        metadata = {"target_directory": "/custom/cargo-output"}
        self.assertEqual(module.build_output(metadata, "debug"), Path("/custom/cargo-output/debug"))
        self.assertEqual(module.build_output(metadata, "release", "x86_64-apple-darwin"), Path("/custom/cargo-output/x86_64-apple-darwin/release"))

    def test_configured_target_never_finalizes_an_old_host_bundle(self):
        for scope in ("project", "tauri", "cargo_home"):
            with self.subTest(scope=scope), tempfile.TemporaryDirectory() as temp:
                root = Path(temp).resolve() / "desktop"
                (root / "src-tauri/resources").mkdir(parents=True)
                for name in ["tauri.conf.json", "resources/desktop.production.toml"]:
                    (root / "src-tauri" / name).write_bytes((ROOT / "src-tauri" / name).read_bytes())
                cargo_home = Path(temp) / "cargo-home"
                config_dir = {"project": root / ".cargo", "tauri": root / "src-tauri/.cargo", "cargo_home": cargo_home}[scope]
                config_dir.mkdir(parents=True)
                (config_dir / "config.toml").write_text('[build]\ntarget = "x86_64-apple-darwin"\n')
                output = root / "custom-output"
                stale = output / "release/bundle/macos/起飞 Test.app"
                stale.mkdir(parents=True)
                expected = output / "x86_64-apple-darwin/release/bundle/macos/起飞 Test.app"
                expected.mkdir(parents=True)
                with (
                    mock.patch.object(module, "__file__", str(root / "scripts/build-desktop-environment.py")),
                    mock.patch.object(module.sys, "argv", ["build", "--environment", "test"]),
                    mock.patch.object(module.sys, "platform", "darwin"),
                    mock.patch.dict(module.os.environ, {"CARGO_HOME": str(cargo_home)}, clear=True),
                    mock.patch.object(module.subprocess, "run", return_value=subprocess.CompletedProcess(
                        [], 0, stdout=json.dumps({"target_directory": str(output)})
                    )) as run,
                    mock.patch.object(module, "finalize_macos") as finalize,
                    mock.patch.object(module, "package_macos_dmg") as dmg,
                ):
                    module.main()
                finalize.assert_called_once_with(root, expected)
                dmg.assert_called_once_with(root, expected, "x86_64-apple-darwin")
                build = next(call.args[0] for call in run.call_args_list if "tauri" in call.args[0])
                self.assertEqual(build[build.index("--target") + 1], "x86_64-apple-darwin")
                metadata_call = next(call for call in run.call_args_list if "metadata" in call.args[0])
                self.assertEqual(metadata_call.kwargs["cwd"], root / "src-tauri")

    def test_cargo_target_precedence_and_legacy_config(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp).resolve()
            cargo_dir = root / "desktop/src-tauri"
            cargo_home = root / "cargo-home"
            for folder, target in [(cargo_home, "home-target"), (root / "desktop/.cargo", "project-target"), (cargo_dir / ".cargo", "tauri-target")]:
                folder.mkdir(parents=True)
                (folder / "config.toml").write_text(f'[build]\ntarget = "{target}"\n')
            env = {"CARGO_HOME": str(cargo_home)}
            self.assertEqual(module.resolve_build_target(cargo_dir, env), "tauri-target")
            (cargo_dir / ".cargo/config").write_text('[build]\ntarget = "legacy-target"\n')
            self.assertEqual(module.resolve_build_target(cargo_dir, env), "legacy-target")
            env["CARGO_BUILD_TARGET"] = "environment-target"
            self.assertEqual(module.resolve_build_target(cargo_dir, env), "environment-target")
            self.assertEqual(module.resolve_build_target(cargo_dir, env, "explicit-target"), "explicit-target")
            del env["CARGO_BUILD_TARGET"]
            (cargo_dir / ".cargo/config").write_text('[build]\ntarget = ["first", "second"]\n')
            with self.assertRaisesRegex(ValueError, "one Cargo target"):
                module.resolve_build_target(cargo_dir, env)

    @unittest.skipUnless(shutil.which("cargo") and shutil.which("rustc"), "Rust toolchain is not installed")
    def test_configured_target_output_matches_real_cargo(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp).resolve()
            cargo_dir = root / "project"
            (cargo_dir / ".cargo").mkdir(parents=True)
            (cargo_dir / "src").mkdir()
            (cargo_dir / "Cargo.toml").write_text('[package]\nname = "target-fixture"\nversion = "0.1.0"\nedition = "2021"\n')
            (cargo_dir / "src/main.rs").write_text('fn main() {}\n')
            host = next(line.split(": ", 1)[1] for line in subprocess.check_output(["rustc", "-vV"], text=True).splitlines() if line.startswith("host: "))
            output = root / "custom-output"
            (cargo_dir / ".cargo/config.toml").write_text(f'[build]\ntarget = "{host}"\ntarget-dir = {json.dumps(str(output))}\n')
            env = dict(module.os.environ, CARGO_HOME=str(root / "cargo-home"))
            env.pop("CARGO_BUILD_TARGET", None)
            env.pop("CARGO_TARGET_DIR", None)
            env.pop("CARGO_BUILD_TARGET_DIR", None)
            metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--offline", "--no-deps", "--format-version", "1"], cwd=cargo_dir, env=env, text=True))
            result = subprocess.run(["cargo", "build", "--offline", "--message-format=json"], cwd=cargo_dir, env=env, capture_output=True, text=True, check=True)
            artifacts = [json.loads(line) for line in result.stdout.splitlines()]
            executable = Path(next(item["executable"] for item in artifacts if item.get("executable")))
            resolved = module.resolve_build_target(cargo_dir, env)
            self.assertEqual(resolved, host)
            self.assertEqual(module.build_output(metadata, "debug", resolved), executable.parent)
            self.assertEqual(executable.parent, output / host / "debug")

    def test_macos_finalization_records_the_signed_sidecar_before_delivery(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            app = root / "起飞 Dev.app"
            (app / "Contents/MacOS").mkdir(parents=True)
            resources = app / "Contents/Resources"
            resources.mkdir()
            binary = app / "Contents/MacOS/wt-media-agent"
            binary.write_bytes(b"unsigned agent")
            manifest = resources / "sidecar-manifest.json"
            manifest.write_text(json.dumps({"filename": "wt-media-agent", "sha256": "old-build-digest"}))
            scripts = root / "scripts"
            scripts.mkdir()
            # Stand in for the existing signing step, which changes Agent bytes
            # and writes the runtime record only after signing is complete.
            (scripts / "repair-macos-signing.sh").write_text("""set -eu
python3 - "$1" <<'RECORD'
from pathlib import Path
import hashlib,json,sys
app=Path(sys.argv[1])
binary=app/'Contents/MacOS/wt-media-agent'
binary.write_bytes(b'signed agent')
record={'filename':'wt-media-agent','sha256':hashlib.sha256(binary.read_bytes()).hexdigest()}
(app/'Contents/Resources/sidecar-manifest.json').write_text(json.dumps(record))
RECORD
""")
            module.finalize_macos(root, app)
            self.assertEqual(binary.read_bytes(), b"signed agent")
            record = json.loads(manifest.read_text())
            self.assertEqual(record["sha256"], __import__("hashlib").sha256(binary.read_bytes()).hexdigest())

    def test_macos_delivery_refuses_a_stale_sidecar_record(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            app = root / "起飞 Test.app"
            (app / "Contents/MacOS").mkdir(parents=True)
            (app / "Contents/Resources").mkdir()
            (app / "Contents/MacOS/wt-media-agent").write_bytes(b"signed agent")
            (app / "Contents/Resources/sidecar-manifest.json").write_text(json.dumps({"filename":"wt-media-agent","sha256":"stale"}))
            (root / "scripts").mkdir()
            (root / "scripts/repair-macos-signing.sh").write_text("exit 0\n")
            with self.assertRaises(ValueError):
                module.finalize_macos(root, app)

    @unittest.skipUnless(shutil.which("makensis"), "NSIS compiler is not installed")
    def test_windows_cleanup_compiles_with_tauri_include_order_and_scopes_every_identity(self):
        # Tauri includes the hooks BEFORE defining PRODUCTNAME. Compile real
        # uninstall code in that order; never execute it or touch user data.
        with tempfile.TemporaryDirectory() as temp:
            folder = Path(temp)
            for name, expected in [("起飞 Dev", "WTMedia\\Dev"), ("起飞 Test", "WTMedia\\Test"), ("起飞", "WTMedia")]:
                fixture = folder / "installer.nsi"
                fixture.write_text(f'''Unicode true
!include LogicLib.nsh
!include FileFunc.nsh
!include "{ROOT / 'src-tauri/windows/installer-hooks.nsh'}"
!define PRODUCTNAME "{name}"
Name "Fixture"
OutFile "{folder / 'fixture.exe'}"
Var DeleteAppDataCheckboxState
Var UpdateMode
Section
WriteUninstaller "$TEMP\\fixture-uninstall.exe"
SectionEnd
Section "Uninstall"
!insertmacro NSIS_HOOK_POSTUNINSTALL
SectionEnd
!if "${{WT_MEDIA_DATA_ROOT}}" != "{expected}"
!error "wrong cleanup namespace"
!endif
''', encoding="utf-8")
                result = subprocess.run(["makensis", "-V2", str(fixture)], capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
