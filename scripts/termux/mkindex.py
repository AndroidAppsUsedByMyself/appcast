#!/usr/bin/env python3
"""Generate repo/windows/index.html for the Windows binaries.

Usage: mkindex.py <version> <windows-dir> <base-url>
"""
import os
import sys


def main() -> None:
    version, win_dir, base = sys.argv[1], sys.argv[2], sys.argv[3]
    files = []
    for f in sorted(os.listdir(win_dir)):
        if f.endswith(".exe"):
            p = os.path.join(win_dir, f)
            sha = open(p + ".sha256").read().strip()
            files.append((f, os.path.getsize(p), sha))
    rows = "\n".join(
        f"<tr><td><a href=\"{base}/{f}\">{f}</a></td><td>{size}B</td><td><code>{sha}</code></td></tr>"
        for f, size, sha in files
    )
    print(f"""<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8">
<title>appcast for Windows</title></head>
<body><h1>appcast for Windows v{version}</h1>
<p>Self-contained executables (statically linked CRT, no extra DLLs). Run from
PowerShell or CMD. On first run Windows SmartScreen may warn about the unsigned
binary &mdash; click <em>More info &gt; Run anyway</em>.</p>
<p>adb and scrcpy are not bundled; install both and keep them on PATH.</p>
<table border="1" cellpadding="6"><tr><th>Binary</th><th>Size</th><th>SHA256</th></tr>
{rows}
</table></body></html>""")


if __name__ == "__main__":
    main()
