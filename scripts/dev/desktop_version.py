#!/usr/bin/env python3
"""Resolve build-time product identity; never reads Workspace or a remote release."""
import argparse
import json
import os
from pathlib import Path
import plistlib
import re
import subprocess

TAG = re.compile(r'v(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:-rc\.[1-9]\d*)?')


def git(root, *args):
    result = subprocess.run(['git', '-C', str(root), *args], capture_output=True, text=True)
    return result.stdout.strip() if result.returncode == 0 else ''


def resolve(root, env):
    base = json.loads((root / 'src-tauri/tauri.conf.json').read_text(encoding='utf-8'))['version']
    tag = env.get('WT_MEDIA_PRODUCT_TAG')
    if env.get('WT_MEDIA_DESKTOP_RELEASE_CHANNEL') == 'stable' and not tag:
        raise ValueError('official Desktop release requires WT_MEDIA_PRODUCT_TAG from the Release Manifest')
    commit = git(root, 'rev-parse', 'HEAD')
    source_tag = env.get('WT_MEDIA_DESKTOP_SOURCE_TAG') or None
    if tag:
        if not TAG.fullmatch(tag):
            raise ValueError('invalid product Tag')
        if not source_tag or not TAG.fullmatch(source_tag) or not commit:
            raise ValueError('official Desktop build requires source Tag and Git commit provenance')
        expected = env.get('WT_MEDIA_DESKTOP_SOURCE_COMMIT')
        if expected and expected != commit:
            raise ValueError('Desktop source commit differs from the release input')
        # CI checks out this exact component tag. Local release entry points must
        # also prove that the supplied tag names the source being packaged.
        tagged = git(root, 'rev-parse', '--verify', source_tag + '^{commit}')
        if tagged != commit:
            raise ValueError('Desktop source Tag does not identify this checkout')
        version = tag[1:]
    else:
        identity = env.get('WT_MEDIA_DESKTOP_ENVIRONMENT', 'dev')
        label = {'dev': 'dev', 'test': 'test', 'prod': 'local'}.get(identity)
        if not label:
            raise ValueError('invalid desktop environment')
        version = base.split('-', 1)[0] + '-' + label + '.' + (commit[:10] or 'unknown')
    dirty = bool(git(root, 'status', '--porcelain', '--untracked-files=no'))
    if not tag and dirty:
        version += '.dirty'
    return {'schema_version': 1, 'product_tag': tag or None, 'desktop_version': version,
            'desktop_source_tag': source_tag, 'desktop_source_commit': commit or None,
            'source_dirty': dirty}


def prepare(root, env):
    record = resolve(root, env)
    generated = root / '.generated'
    generated.mkdir(exist_ok=True)
    metadata = generated / 'desktop-build.json'
    metadata.write_text(json.dumps(record, indent=2) + '\n', encoding='utf-8')
    folder = generated / 'version'
    folder.mkdir(exist_ok=True)
    overlay = folder / 'tauri.conf.json'
    overlay.write_text(json.dumps({'version': record['desktop_version'], 'bundle': {
        'resources': {str(metadata): 'desktop-build.json'}}}, indent=2) + '\n', encoding='utf-8')
    return overlay


def verify_macos(app, tag=None):
    record = json.loads((app / 'Contents/Resources/desktop-build.json').read_text(encoding='utf-8'))
    info = plistlib.loads((app / 'Contents/Info.plist').read_bytes())
    if tag and (record['product_tag'] != tag or record['desktop_version'] != tag[1:]):
        raise ValueError('packaged Desktop product Tag differs from the release input')
    if info.get('CFBundleShortVersionString') != record['desktop_version']:
        raise ValueError('native bundle version differs from embedded Desktop build record')
    return record


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--root', type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument('--prepare', action='store_true')
    parser.add_argument('--field', choices=['desktop_version', 'product_tag', 'desktop_source_tag', 'desktop_source_commit'])
    parser.add_argument('--verify-macos', type=Path)
    args = parser.parse_args()
    try:
        if args.verify_macos:
            value = verify_macos(args.verify_macos, os.environ.get('WT_MEDIA_PRODUCT_TAG'))
            if os.environ.get('WT_MEDIA_PRODUCT_TAG'):
                expected = resolve(args.root, os.environ)
                for key in ('desktop_source_tag', 'desktop_source_commit'):
                    if value.get(key) != expected[key]:
                        raise ValueError('packaged Desktop source differs from the release input')
        elif args.prepare:
            print(prepare(args.root, os.environ)); return
        else:
            value = resolve(args.root, os.environ)
        print(value[args.field] if args.field else json.dumps(value))
    except (ValueError, KeyError, OSError) as exc:
        parser.exit(1, f'Desktop version refused: {exc}\n')


if __name__ == '__main__':
    main()
