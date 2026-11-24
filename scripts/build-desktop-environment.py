"""Build a pinned desktop identity, independently of its Cloud connection."""
import argparse
import hashlib
import json
import os
import platform
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path


def prepare(root, environment, profile):
    if environment not in ('dev', 'test', 'prod') or profile not in ('debug', 'release'):
        raise ValueError('invalid desktop build identity')
    if profile == 'release' and environment == 'dev':
        raise ValueError('release desktop must be test or prod')
    name = {'dev': '起飞 Dev', 'test': '起飞 Test', 'prod': '起飞'}[environment]
    suffix = '' if environment == 'prod' else '.' + environment
    icon = 'icons' if environment == 'prod' else 'icons/' + environment
    base = json.loads((root / 'src-tauri/tauri.conf.json').read_text(encoding="utf-8"))
    windows = base['app']['windows']
    windows[0]['title'] = name + ' · 内容运营平台'
    folder = root / '.generated/environment' / environment
    folder.mkdir(parents=True, exist_ok=True)
    source = (root / 'src-tauri/resources/desktop.production.toml').read_text(encoding="utf-8")
    config = source.replace('environment = "prod"', 'environment = "' + environment + '"', 1)
    runtime = folder / 'desktop.production.toml'
    runtime.write_text(config, encoding="utf-8")
    assert tomllib.loads(config)['environment'] == environment
    overlay = {
        'mainBinaryName': 'wt-media-desktop-shell' if environment == 'prod' else 'wt-media-desktop.' + environment,
        'identifier': 'com.wtmedia.desktop' + suffix,
        'productName': name,
        'app': {'windows': windows},
        'bundle': {
            'icon': [icon + '/icon.png', icon + '/icon.icns', icon + '/icon.ico'],
            'resources': {
                'resources/desktop.production.toml': None,
                str(runtime): 'resources/desktop.production.toml',
            },
        },
    }
    target = folder / 'tauri.conf.json'
    target.write_text(json.dumps(overlay, ensure_ascii=False, indent=2) + '\n', encoding="utf-8")
    return target


def finalize_macos(root, app):
    # Tauri signing rewrites the embedded Agent. Reuse the established release
    # order: repair the child signature, record its final bytes, seal the app.
    subprocess.run(['bash', str(root / 'scripts/repair-macos-signing.sh'), str(app)], check=True)
    sidecar = app / 'Contents/MacOS/wt-media-agent'
    record = json.loads((app / 'Contents/Resources/sidecar-manifest.json').read_text(encoding="utf-8"))
    if record.get('filename') != 'wt-media-agent' or record.get('sha256') != hashlib.sha256(sidecar.read_bytes()).hexdigest():
        raise ValueError('packaged Agent differs from its final runtime record')


def resolve_build_target(cargo_dir, env, explicit=None):
    """Resolve one target using Cargo's directory, home and override precedence."""
    if explicit or env.get('CARGO_BUILD_TARGET'):
        return explicit or env['CARGO_BUILD_TARGET']
    cargo_dir = cargo_dir.resolve()
    cargo_home = Path(env.get('CARGO_HOME') or Path.home() / '.cargo').resolve()
    directories = [cargo_home, *(parent / '.cargo' for parent in reversed(cargo_dir.parents)), cargo_dir / '.cargo']
    target = None
    seen = set()
    for directory in directories:
        if directory in seen:
            continue
        seen.add(directory)
        legacy = directory / 'config'
        config = legacy if legacy.is_file() else directory / 'config.toml'
        if not config.is_file():
            continue
        settings = tomllib.loads(config.read_text(encoding="utf-8"))
        if settings.get('include'):
            raise ValueError('Cargo config includes require an explicit --target for Desktop packaging')
        value = settings.get('build', {}).get('target')
        if value is not None:
            if isinstance(value, list) and isinstance(target, list):
                target = target + value
            else:
                target = value
    if isinstance(target, list):
        if len(target) != 1:
            raise ValueError('Desktop packaging requires one Cargo target; pass --target explicitly')
        target = target[0]
    if target is not None and (not isinstance(target, str) or not target):
        raise ValueError('Cargo build.target must specify a nonempty target')
    return target


def build_output(metadata, profile, target=None):
    directory = Path(metadata['target_directory'])
    return directory / target / profile if target else directory / profile


def package_macos_dmg(root, app, target_triple=None):
    # Create the disk image only from the finalized app. This also avoids the
    # Finder automation used by Tauri's default DMG aesthetic customization.
    base = json.loads((root / 'src-tauri/tauri.conf.json').read_text(encoding="utf-8"))
    architecture = target_triple.split('-')[0] if target_triple else platform.machine()
    arch = {'arm64': 'aarch64', 'aarch64': 'aarch64', 'x86_64': 'x64', 'universal': 'universal'}[architecture]
    target = app.parent.parent / 'dmg' / f'{app.stem}_{base["version"]}_{arch}.dmg'
    target.parent.mkdir(parents=True, exist_ok=True)
    # Verify a fresh image path before replacing an older output. macOS keeps
    # mounted images open, so overwriting an attached output in place would make
    # verification fail with a resource-busy error.
    with tempfile.TemporaryDirectory(prefix='wt-media-environment-dmg-', dir=target.parent) as temporary:
        working = Path(temporary)
        staging = working / 'contents'
        staging.mkdir()
        image = working / target.name
        subprocess.run(['ditto', str(app), str(staging / app.name)], check=True)
        (staging / 'Applications').symlink_to('/Applications')
        subprocess.run(['hdiutil', 'create', '-volname', app.stem, '-srcfolder', str(staging), '-format', 'UDZO', str(image)], check=True)
        subprocess.run(['hdiutil', 'verify', str(image)], check=True)
        image.replace(target)
    print(f'Verified disk image: {target}')


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--environment', choices=['dev', 'test', 'prod'], required=True)
    parser.add_argument('--profile', choices=['debug', 'release'], default='release')
    parser.add_argument('--target', help='Rust target triple')
    parser.add_argument('--bundles', help='Bundle formats, comma separated (for example app,dmg)')
    parser.add_argument('--dev', action='store_true', help='Run the development window instead of packaging')
    parser.add_argument('--dry-run', action='store_true')
    args, extra = parser.parse_known_args()
    root = Path(__file__).resolve().parents[1]
    if args.dev and (args.profile != 'debug' or args.environment != 'dev'):
        parser.error('--dev requires --profile debug --environment dev')
    config = prepare(root, args.environment, args.profile)
    env = os.environ.copy()
    env['WT_MEDIA_DESKTOP_ENVIRONMENT'] = args.environment
    if args.profile == 'release':
        env['WT_MEDIA_DESKTOP_RELEASE_CHANNEL'] = 'stable' if args.environment == 'prod' else 'test'
    else:
        env.pop('WT_MEDIA_DESKTOP_RELEASE_CHANNEL', None)
    # Tauri invokes Cargo inside src-tauri, so use that config hierarchy for
    # both target resolution and metadata (including build.target-dir).
    cargo_dir = root / 'src-tauri'
    target_triple = resolve_build_target(cargo_dir, env, args.target)
    command = ['cargo', 'tauri', 'dev' if args.dev else 'build', '--config', str(config)]
    if target_triple:
        command.extend(['--target', target_triple])
    if args.profile == 'debug' and not args.dev:
        command.append('--debug')
    macos_package = sys.platform == 'darwin' and not args.dev
    requested_bundles = set((args.bundles or 'all').split(','))
    create_dmg = macos_package and bool(requested_bundles & {'all', 'dmg'})
    if create_dmg:
        command.extend(['--bundles', 'app'])
    elif args.bundles:
        command.extend(['--bundles', args.bundles])
    command.extend(extra)
    if args.dry_run:
        print(json.dumps({'command': command, 'desktop_environment': args.environment, 'profile': args.profile, 'finalize_macos': macos_package, 'create_dmg': create_dmg}, ensure_ascii=False))
        return
    output = None
    if macos_package:
        metadata = subprocess.run(['cargo', 'metadata', '--no-deps', '--format-version', '1'], cwd=cargo_dir, env=env, check=True, capture_output=True, text=True)
        output = build_output(json.loads(metadata.stdout), args.profile, target_triple)
    subprocess.run(command, cwd=root, env=env, check=True)
    if macos_package:
        name = json.loads(config.read_text(encoding="utf-8"))['productName']
        app = output / 'bundle/macos' / (name + '.app')
        finalize_macos(root, app)
        if create_dmg:
            package_macos_dmg(root, app, target_triple)


if __name__ == '__main__':
    main()
