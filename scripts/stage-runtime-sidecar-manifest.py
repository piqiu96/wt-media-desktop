#!/usr/bin/env python3
"""Write the stable runtime Sidecar record before Tauri bundles resources."""

from __future__ import annotations

import argparse
import json
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--build-manifest", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    source = json.loads(args.build_manifest.read_text(encoding="utf-8"))
    record = {key: source[key] for key in ("component", "version", "target", "sha256")}
    record["filename"] = "wt-media-agent"
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(record, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
