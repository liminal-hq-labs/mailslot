#!/usr/bin/env python3
# Builds the sticky pull request comment from JUnit XML files and the commit's check runs.
#
# (c) Copyright 2026 Liminal HQ, Scott Morris
# SPDX-License-Identifier: Apache-2.0 OR MIT

"""Usage: junit-comment.py --sha SHA [--checks checks.json] [--run-url URL] FILE.xml...

Prints Markdown: pass, fail and skip counts per suite, the failing test names with an excerpt of
each failure, and the status of each check run on the commit. The first line is a hidden marker,
so the publishing workflow can find and update the same comment.
"""

import argparse
import json
import sys
import xml.etree.ElementTree as ET

MARKER = "<!-- mailslot-test-results -->"
EXCERPT_LINES = 12
MAX_FAILURES = 15


def suite_stats(path):
    root = ET.parse(path).getroot()
    suites = [root] if root.tag == "testsuite" else list(root.iter("testsuite"))
    tests = failures = errors = skipped = 0
    failing = []
    for suite in suites:
        for case in suite.iter("testcase"):
            tests += 1
            name = f'{case.get("classname", "")}::{case.get("name", "")}'.strip(":")
            problem = case.find("failure")
            if problem is None:
                problem = case.find("error")
            if case.find("skipped") is not None:
                skipped += 1
            elif problem is not None:
                if problem.tag == "failure":
                    failures += 1
                else:
                    errors += 1
                text = (problem.get("message") or "") + "\n" + (problem.text or "")
                lines = [line for line in text.strip().splitlines() if line.strip()]
                failing.append((name, "\n".join(lines[:EXCERPT_LINES])))
    return tests, failures, errors, skipped, failing


def label(run):
    conclusion = run.get("conclusion")
    if conclusion == "success":
        return "passed"
    if conclusion in ("failure", "timed_out"):
        return "**failed**"
    if conclusion in ("cancelled", "skipped", "neutral"):
        return conclusion
    return run.get("status") or "pending"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--sha", required=True)
    parser.add_argument("--checks")
    parser.add_argument("--run-url")
    parser.add_argument("files", nargs="*")
    args = parser.parse_args()

    out = [MARKER, "## Mailslot test results", ""]
    all_failing = []
    rows = []
    for path in args.files:
        try:
            tests, failures, errors, skipped, failing = suite_stats(path)
        except (ET.ParseError, OSError) as exc:
            rows.append(f"| `{path}` | could not be read: {exc} | | | |")
            continue
        passed = tests - failures - errors - skipped
        rows.append(f"| `{path.split('/')[-2] if path.count('/') >= 1 else path}` | {tests} | {passed} | {failures + errors} | {skipped} |")
        all_failing.extend(failing)

    if rows:
        out += ["| Suite | Tests | Passed | Failed | Skipped |", "|---|---|---|---|---|", *rows, ""]
    else:
        out += ["No test results were uploaded for this run.", ""]

    if all_failing:
        out += [f"### Failing tests ({len(all_failing)})", ""]
        for name, excerpt in all_failing[:MAX_FAILURES]:
            out += [f"<details><summary><code>{name}</code></summary>", "", "```text", excerpt, "```", "", "</details>", ""]
        if len(all_failing) > MAX_FAILURES:
            out += [f"…and {len(all_failing) - MAX_FAILURES} more.", ""]

    if args.checks:
        try:
            runs = json.load(open(args.checks))
        except (OSError, json.JSONDecodeError):
            runs = []
        runs = sorted(runs, key=lambda r: r.get("name", ""))
        if runs:
            out += ["### Checks", "", "| Check | Status |", "|---|---|"]
            for run in runs:
                out.append(f'| {run.get("name")} | {label(run)} |')
            out.append("")

    footer = f"Commit `{args.sha[:7]}`"
    if args.run_url:
        footer += f" · [workflow run]({args.run_url})"
    out += [footer, ""]
    sys.stdout.write("\n".join(out))


if __name__ == "__main__":
    main()
