#!/usr/bin/env python3
"""Generate per-plugin hosting pages under <out>/<crate>/index.html plus a
top-level <out>/index.html, from a release manifest.json.

Plugin release assets follow the loader's file-name convention
(libappcast_tpt_<name>-<target>.so|.dll), so a plain curl into
~/.config/appcast/transporters/ is already a working install — the pages
just hand out that one-liner per platform.

Usage: mkplugins.py --manifest FILE --out DIR
"""
import argparse
import json
import os
import re
import sys

ASSET_RE = re.compile(
    r"^lib(?P<crate>appcast_tpt_[a-z0-9_]+)-(?P<target>.+)\.(?P<ext>so|dll)$"
)

INSTALL = {
    "linux": (
        "mkdir -p ~/.config/appcast/transporters\n"
        "curl -fL {url} -o ~/.config/appcast/transporters/{filename}"
    ),
    "windows": (
        "mkdir \"$env:APPDATA\\appcast\\transporters\" -Force\n"
        "curl.exe -fL \"{url}\" "
        "-o \"$env:APPDATA\\appcast\\transporters\\{filename}\""
    ),
}


def esc(s: str) -> str:
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def human(n: int) -> str:
    return f"{n / 1024:.0f} KiB" if n < 1024 * 1024 else f"{n / 1024 / 1024:.1f} MiB"


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--manifest", required=True)
    ap.add_argument("--out", required=True)
    args = ap.parse_args()

    manifest = json.load(open(args.manifest))
    version, tag = manifest["version"], manifest["tag"]

    # crate id -> target -> asset dict (sha256 sidecars are excluded from
    # the manifest itself; verify against "<asset>.sha256" next to it)
    plugins: dict[str, dict[str, dict]] = {}
    for label, asset in sorted(manifest["assets"].items()):
        m = ASSET_RE.match(asset["filename"])
        if not m:
            continue
        entry = dict(asset)
        entry["sha256"] = None  # filled below when the sidecar exists
        for cand in (label + ".sha256", asset["filename"] + ".sha256"):
            if cand in manifest["assets"]:
                entry["sha256"] = manifest["assets"][cand]["sha256"]
                break
        plugins.setdefault(m.group("crate"), {})[m.group("target")] = entry

    if not plugins:
        print("no plugin assets in manifest; skipping", file=sys.stderr)
        return

    os.makedirs(args.out, exist_ok=True)
    links = []
    for crate, targets in sorted(plugins.items()):
        crate_dir = os.path.join(args.out, crate)
        os.makedirs(crate_dir, exist_ok=True)

        rows = []
        for target, a in sorted(targets.items()):
            sha = a["sha256"] or "see sidecar"
            plat = "Windows" if a["filename"].endswith(".dll") else "Linux"
            cmd = INSTALL["windows" if plat == "Windows" else "linux"].format(
                url=a["url"], filename=a["filename"]
            )
            rows.append(f"""<tr><td>{plat} ({esc(target)})</td>
<td><a href="{a['url']}"><code>{esc(a['filename'])}</code></a></td>
<td>{human(a['size'])}</td><td><code>{esc(sha[:16])}&hellip;</code></td>
<td><pre>{esc(cmd)}</pre></td></tr>""")

        html = f"""<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8">
<title>appcast plugin: {esc(crate)}</title></head>
<body><h1>appcast plugin: <code>{esc(crate)}</code> v{esc(version)}</h1>
<p>Drop the artifact for your platform into
<code>~/.config/appcast/transporters/</code> (Windows:
<code>%APPDATA%\\appcast\\transporters</code> &mdash; the literal
<code>~\\.config</code> path is scanned too) &mdash; the
file names below already match what the loader scans, no renaming needed.
Run <code>appcast transporters</code> afterwards to see it listed.</p>
<table border="1" cellpadding="6">
<tr><th>Platform</th><th>Artifact</th><th>Size</th><th>SHA256</th><th>Install</th></tr>
{''.join(rows)}
</table>
<p>Source: release <a href="https://github.com/{manifest.get('repo', 'AndroidAppsUsedByMyself/appcast')}/releases/tag/{esc(tag)}">{esc(tag)}</a>.
Verify downloads against the published <code>.sha256</code> sidecars.</p>
</body></html>"""
        with open(os.path.join(crate_dir, "index.html"), "w") as fh:
            fh.write(html)

        links.append(
            f'<li><a href="{crate}/index.html"><code>{esc(crate)}</code></a> '
            f"&mdash; {len(targets)} platform(s), v{esc(version)}</li>"
        )

    index_html = f"""<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8">
<title>appcast transporter plugins</title></head>
<body><h1>appcast transporter plugins v{esc(version)}</h1>
<ul>{''.join(links)}</ul>
<p>Plugins extend appcast with additional transporters at runtime; see the
repository README for authoring your own via the appcast-plugin SDK.</p>
</body></html>"""
    with open(os.path.join(args.out, "index.html"), "w") as fh:
        fh.write(index_html)

    print(f"wrote {len(plugins)} plugin page(s) under {args.out}")


if __name__ == "__main__":
    main()
