#!/usr/bin/env python3
"""Pin the Windows-only GUI subsystem declaration without changing debug builds."""

import re
from pathlib import Path


SOURCE = Path(__file__).parent.parent / "src-tauri" / "src" / "main.rs"
EXPECTED = r"""#!\[cfg_attr\(
\s*all\(target_os\s*=\s*"windows",\s*not\(debug_assertions\)\),
\s*windows_subsystem\s*=\s*"windows"
\)\]"""


def main() -> int:
    source = SOURCE.read_text(encoding="utf-8")
    if not re.search(EXPECTED, source):
        print("missing conditional Windows GUI subsystem declaration")
        return 1
    if re.search(r"#!\[windows_subsystem", source):
        print("found an unconditional Windows GUI subsystem declaration")
        return 1
    print("Windows GUI subsystem is release-only")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
