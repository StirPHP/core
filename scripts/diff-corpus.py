#!/usr/bin/env python3
"""Compares two runs of scripts/check-corpus.sh, by diagnostic.

    scripts/diff-corpus.py <old-label> <new-label> [project...]

Exits with 1 if any diagnostic differs.
"""
import collections
import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent / ".corpus-results"
SHOWN = 12


def load(label, project):
    path = ROOT / label / f"{project}.json"
    with open(path, encoding="utf-8") as f:
        items = json.load(f)
    return collections.Counter(
        (d["file"], d["line"], d["code"], d["message"]) for d in items
    )


def summarize(title, diff):
    print(f"  {title}: {sum(diff.values())}")
    groups = collections.Counter()
    for (_, _, code, message), n in diff.items():
        groups[(code, re.sub(r"`[^`]*`", "`_`", message)[:90])] += n
    for (code, message), n in groups.most_common(SHOWN):
        print(f"    {n:6} {code} {message}")


def main():
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    old, new = sys.argv[1:3]
    projects = sys.argv[3:] or sorted(p.stem for p in (ROOT / new).glob("*.json"))
    changed = False
    for project in projects:
        before, after = load(old, project), load(new, project)
        gone, added = before - after, after - before
        print(f"{project}: {sum(before.values())} -> {sum(after.values())}")
        if gone or added:
            changed = True
            summarize("gone", gone)
            summarize("new", added)
    sys.exit(1 if changed else 0)


if __name__ == "__main__":
    main()
