"""Packaging identity and resource routing regression, without building apps."""
import importlib.util
import json
import shutil
import subprocess
import tempfile
import unittest
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
                runtime = next(Path(path) for path in resources if path != "resources/desktop.production.toml")
                config = module.tomllib.loads(runtime.read_text())
                self.assertEqual(config["environment"], env)
                self.assertEqual(config["cloud"]["cloud_env"], "prod")

    def test_release_cannot_accidentally_package_development_identity(self):
        with self.assertRaises(ValueError):
            module.prepare(ROOT, "dev", "release")

    def test_local_formal_entry_pins_release_gate(self):
        self.assertIn("export WT_MEDIA_DESKTOP_RELEASE_CHANNEL=stable", (ROOT / "scripts/build-release-macos.sh").read_text())

    def test_finalization_follows_cargo_target_directory_and_explicit_target(self):
        metadata = {"target_directory": "/custom/cargo-output"}
        self.assertEqual(module.build_output(metadata, "debug"), Path("/custom/cargo-output/debug"))
        self.assertEqual(module.build_output(metadata, "release", "x86_64-apple-darwin"), Path("/custom/cargo-output/x86_64-apple-darwin/release"))

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
