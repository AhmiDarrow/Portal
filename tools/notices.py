"""Writes the licence notices for the Rust crates built into Portal.

    python tools/notices.py <output file>

Reads `cargo metadata` for the Windows target, walks what portal-http and
portal-launcher link (build-only and test-only crates are left out), and
collects each crate's licence files from its downloaded source. Identical
texts are printed once, under every crate that carries them.
"""

from __future__ import annotations

import hashlib
import json
import subprocess
import sys
from pathlib import Path

ROOTS = ("portal-http", "portal-launcher")
TARGET = "x86_64-pc-windows-msvc"
NAMES = ("license", "licence", "copying", "notice", "unlicense", "copyright")


def metadata(root: Path) -> dict:
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", TARGET],
        cwd=root,
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    return json.loads(out.stdout)


def linked(meta: dict) -> list[dict]:
    """The packages the two programs link, apart from Portal's own."""
    packages = {package["id"]: package for package in meta["packages"]}
    nodes = {node["id"]: node for node in meta["resolve"]["nodes"]}
    own = set(meta["workspace_members"])
    todo = [pid for pid in own if packages[pid]["name"] in ROOTS]
    seen: set[str] = set()
    while todo:
        pid = todo.pop()
        if pid in seen:
            continue
        seen.add(pid)
        for dep in nodes[pid]["deps"]:
            kinds = {kind.get("kind") for kind in dep.get("dep_kinds", [])}
            # None is an ordinary dependency; "build" and "dev" are not linked.
            if None in kinds:
                todo.append(dep["pkg"])
    found = [packages[pid] for pid in seen if pid not in own]
    return sorted(found, key=lambda package: (package["name"].lower(), package["version"]))


def licence_files(package: dict) -> list[Path]:
    folder = Path(package["manifest_path"]).parent
    files = [
        path
        for path in sorted(folder.iterdir())
        if path.is_file() and path.name.lower().startswith(NAMES)
    ]
    named = package.get("license_file")
    if named and (folder / named).is_file() and (folder / named) not in files:
        files.append(folder / named)
    return files


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    root = Path(__file__).resolve().parent.parent
    texts: dict[str, str] = {}
    users: dict[str, list[str]] = {}
    rows: list[str] = []
    bare: list[str] = []
    for package in linked(metadata(root)):
        label = f"{package['name']} {package['version']}"
        licence = package.get("license") or "see its licence file"
        home = package.get("repository") or package.get("homepage") or ""
        rows.append(f"  {label}  ({licence})  {home}".rstrip())
        files = licence_files(package)
        if not files:
            bare.append(label)
        for path in files:
            text = path.read_text(encoding="utf-8", errors="replace").replace("\r\n", "\n").strip()
            if not text:
                continue
            key = hashlib.sha256(" ".join(text.split()).encode("utf-8")).hexdigest()
            texts.setdefault(key, text)
            users.setdefault(key, [])
            if label not in users[key]:
                users[key].append(label)
    if not rows:
        print("No linked crates were found.", file=sys.stderr)
        return 1
    rule = "=" * 78
    out = [
        "Portal third-party notices",
        "",
        "portal.exe and portal-launcher.exe are built with the open-source Rust",
        "crates listed here. Each is used under the licence named beside it, and",
        "the licence texts those crates carry follow the list.",
        "",
        "hls.js has its own notice in vendor\\hls.js.LICENSE.txt, and the Remedy Sans",
        "and Ahmi Pixel typefaces in the two *.LICENSE.txt files beside it. ffmpeg, Kodi and",
        "Python are separate programs that setup downloads; each comes with its own.",
        "",
        *rows,
    ]
    if bare:
        out += ["", "These crates ship no licence file of their own; the licence named above applies:", ""]
        out += [f"  {label}" for label in bare]
    for key, text in sorted(texts.items(), key=lambda item: users[item[0]][0].lower()):
        out += ["", rule, "Used by: " + ", ".join(users[key]), rule, "", text]
    Path(sys.argv[1]).write_text("\n".join(out) + "\n", encoding="utf-8", newline="\r\n")
    print(f"    {len(rows)} crates, {len(texts)} licence texts")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
