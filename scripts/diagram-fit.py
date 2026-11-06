#!/usr/bin/env python3
# Renders each diagram in headless Chromium and checks text fit, collisions and motion.
#
# (c) Copyright 2026 Liminal HQ, Scott Morris
# SPDX-License-Identifier: Apache-2.0 OR MIT

"""Usage: diagram-fit.py [--shots DIR] [--times T,T,...] FILE.svg...

Inlines every diagram into a page with scripts/diagram-fit.js, loads it with `--dump-dom` and
reads the JSON report back. The script samples the animation every quarter second through one
loop, in light and dark and with reduced motion, with the native fonts and again with DejaVu Sans
and Liberation Sans, and fails when a label comes within 4 px of its box, crosses another box,
overlaps another label or sits on a line. Exits 1 on any failure.

With --shots DIR it also writes PNG frames (light, dark and reduced motion) for visual review.
Set CHROMIUM to pick the browser binary.
"""

import html
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
CANDIDATES = ["chromium", "chromium-browser", "google-chrome", "google-chrome-stable", "chrome"]


def find_browser():
    explicit = os.environ.get("CHROMIUM")
    if explicit:
        return explicit if shutil.which(explicit) else None
    for name in CANDIDATES:
        path = shutil.which(name)
        if path:
            return path
    return None


def page(svg_text, script):
    svg_text = re.sub(r"^\s*<\?xml[^>]*\?>", "", svg_text)
    return (
        "<!doctype html><html><head><meta charset=\"utf-8\">"
        "<style>html,body{margin:0;padding:0;background:#ffffff}body>svg{display:block}</style>"
        f"</head><body>{svg_text}<pre id=\"diagram-fit\"></pre><script>{script}</script></body></html>"
    )


def browser_args(browser, extra):
    args = [
        browser,
        "--headless=new",
        "--disable-gpu",
        "--no-first-run",
        "--no-default-browser-check",
        "--hide-scrollbars",
        "--force-device-scale-factor=1",
        "--virtual-time-budget=20000",
    ]
    # Containers and CI runners often lack the user namespaces Chromium's sandbox needs.
    if os.environ.get("CI") or os.geteuid() == 0:
        args.append("--no-sandbox")
    return args + extra


def size_of(svg_text):
    m = re.search(r'viewBox="\s*[-\d.]+[ ,]+[-\d.]+[ ,]+([\d.]+)[ ,]+([\d.]+)\s*"', svg_text)
    return (int(float(m.group(1))), int(float(m.group(2)))) if m else (960, 540)


def check(browser, svg, workdir, script):
    text = svg.read_text(encoding="utf-8")
    harness = workdir / f"{svg.stem}.html"
    harness.write_text(page(text, script), encoding="utf-8")
    profile = workdir / f"profile-{svg.stem}"
    run = subprocess.run(
        browser_args(browser, [f"--user-data-dir={profile}", "--dump-dom", harness.as_uri()]),
        capture_output=True,
        text=True,
        timeout=180,
    )
    m = re.search(r'<pre id="diagram-fit">(.*?)</pre>', run.stdout, re.S)
    if not m or not m.group(1).strip():
        return {"error": f"no report from the browser (exit {run.returncode}): {run.stderr.strip()[-400:]}"}
    return json.loads(html.unescape(m.group(1)))


def shots(browser, svg, workdir, outdir, times, script):
    text = svg.read_text(encoding="utf-8")
    width, height = size_of(text)
    harness = workdir / f"{svg.stem}.html"
    harness.write_text(page(text, script), encoding="utf-8")
    profile = workdir / f"profile-{svg.stem}"
    frames = [("light", "0", t) for t in times] + [("dark", "0", t) for t in times]
    frames += [("light", "1", 0), ("dark", "1", 0)]
    for scheme, reduced, t in frames:
        name = f"{svg.stem}-{scheme}{'-reduced' if reduced == '1' else ''}-t{t:05.2f}.png"
        url = f"{harness.as_uri()}?shot=1&scheme={scheme}&reduced={reduced}&t={t}"
        subprocess.run(
            browser_args(
                browser,
                [
                    f"--user-data-dir={profile}",
                    f"--window-size={width},{height}",
                    f"--screenshot={outdir / name}",
                    url,
                ],
            ),
            capture_output=True,
            timeout=180,
            check=False,
        )


def main(argv):
    shots_dir = None
    times = None
    files = []
    it = iter(argv)
    for arg in it:
        if arg == "--shots":
            shots_dir = Path(next(it))
        elif arg == "--times":
            times = [float(t) for t in next(it).split(",")]
        else:
            files.append(Path(arg))
    if not files:
        print(__doc__, file=sys.stderr)
        return 2
    browser = find_browser()
    if not browser:
        print("diagram-fit: no Chromium or Chrome found (set CHROMIUM)", file=sys.stderr)
        return 3
    script = (HERE / "diagram-fit.js").read_text(encoding="utf-8")
    failed = False
    with tempfile.TemporaryDirectory(prefix="diagram-fit-") as tmp:
        workdir = Path(tmp)
        for svg in files:
            report = check(browser, svg, workdir, script)
            if "error" in report:
                print(f"diagram-fit: FAIL: {svg}: {report['error']}")
                failed = True
                continue
            s = report["stats"]
            summary = (
                f"{report['texts']} labels, {report['boxes']} boxes, {report['lines']} lines, "
                f"{s['frames']} frames, {s['measurements']} measurements; "
                f"worst margin {s['worstX']:.1f} px across ({s['worstXAt']}), "
                f"{s['worstY']:.1f} px down; lowest contrast {s['minContrast']:.2f}"
            )
            if report["failures"]:
                failed = True
                print(f"diagram-fit: FAIL: {svg}: {summary}")
                for f in report["failures"]:
                    print(f"  - {f}")
            else:
                print(f"diagram-fit: ok: {svg}: {summary}")
            if "DejaVu Sans" not in report["fonts"]:
                print(f"diagram-fit: note: DejaVu Sans is not installed, so the wide-font pass fell back (fonts: {', '.join(report['fonts']) or 'none'})")
            if shots_dir:
                shots_dir.mkdir(parents=True, exist_ok=True)
                loop = report["loop"] or 1
                frame_times = times or [round(loop * i / 6, 2) for i in range(6)]
                shots(browser, svg, workdir, shots_dir, frame_times, script)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
