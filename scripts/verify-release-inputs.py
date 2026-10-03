#!/usr/bin/env python3
"""Fail before Tauri compilation if the staged Sidecar and Web are inconsistent."""

from __future__ import annotations

import argparse
import hashlib
import json
import tomllib
from pathlib import Path


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def verify(manifest_path: Path, binary_dir: Path, frontend: Path, agent_config: Path) -> None:
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    binary = binary_dir / manifest["filename"]
    if not binary.is_file() or sha256(binary) != manifest["sha256"]:
        raise ValueError("staged Sidecar missing or SHA-256 differs from Agent manifest")
    marker_path = frontend / "frontend-build.json"
    marker = json.loads(marker_path.read_text(encoding="utf-8"))
    actual = {
        file.relative_to(frontend).as_posix(): sha256(file)
        for file in sorted(frontend.rglob("*"))
        if file.is_file() and file != marker_path
    }
    lines = "".join(f"{name} {digest}\n" for name, digest in sorted(actual.items()))
    if (not (frontend / "index.html").is_file() or marker["manifest"] != actual
            or marker["digest"] != hashlib.sha256(lines.encode()).hexdigest()):
        raise ValueError("Desktop Web differs from Cloud build marker")
    agent = tomllib.loads(agent_config.read_text(encoding="utf-8"))
    if not agent.get("cloud", {}).get("base_url"):
        raise ValueError("staged Agent config has no Cloud origin")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest", required=True, type=Path)
    parser.add_argument("--binary-dir", required=True, type=Path)
    parser.add_argument("--frontend", required=True, type=Path)
    parser.add_argument("--agent-config", required=True, type=Path)
    args = parser.parse_args()
    try:
        verify(args.manifest, args.binary_dir, args.frontend, args.agent_config)
    except (OSError, ValueError, KeyError, json.JSONDecodeError, tomllib.TOMLDecodeError) as exc:
        parser.exit(1, f"release inputs invalid: {exc}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
