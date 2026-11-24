"""Product versions must be independent of the pinned Desktop source tag."""
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts/dev'))
from desktop_version import resolve, prepare, verify_macos

class DesktopVersionTests(unittest.TestCase):
    def fixture(self, root):
        (root / 'src-tauri').mkdir()
        (root / 'src-tauri/tauri.conf.json').write_text('{"version":"0.1.0"}')
        subprocess.run(['git','init','-q',str(root)], check=True)
        subprocess.run(['git','-C',str(root),'add','.'], check=True)
        subprocess.run(['git','-C',str(root),'-c','user.name=Test','-c','user.email=test@example.invalid','commit','-qm','fixture'], check=True)
        subprocess.run(['git','-C',str(root),'tag','v0.1.0-rc.9'], check=True)
        return {'WT_MEDIA_PRODUCT_TAG':'v0.1.2', 'WT_MEDIA_DESKTOP_SOURCE_TAG':'v0.1.0-rc.9', 'WT_MEDIA_DESKTOP_RELEASE_CHANNEL':'stable'}

    def test_one_source_can_build_two_product_versions_without_editing_source(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp); env=self.fixture(root)
            one=resolve(root, env)
            env['WT_MEDIA_PRODUCT_TAG']='v0.1.3'
            two=resolve(root, env)
            self.assertEqual(one['desktop_version'],'0.1.2')
            self.assertEqual(two['desktop_version'],'0.1.3')
            self.assertEqual(one['desktop_source_commit'],two['desktop_source_commit'])
            self.assertEqual(one['desktop_source_tag'],'v0.1.0-rc.9')
            self.assertEqual(json.loads((root/'src-tauri/tauri.conf.json').read_text())['version'],'0.1.0')

    def test_official_build_requires_valid_product_tag_and_source_provenance(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp); env=self.fixture(root)
            for tag in ['', 'v1.2', 'v01.2.3', '1.2.3', 'v1.2.3;bad']:
                with self.subTest(tag=tag), self.assertRaises(ValueError):
                    resolve(root, dict(env, WT_MEDIA_PRODUCT_TAG=tag))
            with self.assertRaises(ValueError):
                resolve(root, dict(env, WT_MEDIA_DESKTOP_SOURCE_COMMIT='b'*40))
            with self.assertRaises(ValueError):
                resolve(root, dict(env, WT_MEDIA_DESKTOP_SOURCE_TAG=''))

    def test_unpublished_build_is_identified_and_never_reuses_stale_release_metadata(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp); env=self.fixture(root)
            prepare(root,env)
            record=resolve(root, {'WT_MEDIA_DESKTOP_ENVIRONMENT':'test'})
            self.assertIsNone(record['product_tag'])
            self.assertIn('-test.',record['desktop_version'])
            self.assertNotEqual(record['desktop_version'],'0.1.2')

    def test_cli_overlay_and_embedded_record_share_one_version(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp); env=self.fixture(root)
            config=json.loads(prepare(root,env).read_text())
            self.assertEqual(config['version'],'0.1.2')
            record=json.loads((root/'.generated/desktop-build.json').read_text())
            self.assertEqual(record['desktop_version'],config['version'])
            self.assertEqual(record['product_tag'],'v0.1.2')
            self.assertIn('desktop-build.json',config['bundle']['resources'].values())

    def test_rejects_native_bundle_version_different_from_embedded_record(self):
        import plistlib
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp); env=self.fixture(root); prepare(root,env)
            app=root/'app.app'; resources=app/'Contents/Resources'; resources.mkdir(parents=True)
            (resources/'desktop-build.json').write_bytes((root/'.generated/desktop-build.json').read_bytes())
            plist=app/'Contents/Info.plist'
            plist.write_bytes(plistlib.dumps({'CFBundleShortVersionString':'0.1.2'}))
            verify_macos(app, 'v0.1.2')
            plist.write_bytes(plistlib.dumps({'CFBundleShortVersionString':'0.1.0'}))
            with self.assertRaises(ValueError): verify_macos(app,'v0.1.2')

if __name__=='__main__': unittest.main()
