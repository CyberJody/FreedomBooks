#!/usr/bin/env python3
"""Generate THIRD-PARTY-NOTICES.md for a FreedomBooks v2 release.

Lists every third-party Rust crate and npm package that ends up in the
shipped desktop app, with its declared license and the copyright line(s)
found in its license files, grouped by license expression, followed by one
copy of each license text.

What counts as "shipped":
  * Rust: the dependency closure (normal + build, NOT dev) of the crates
    named by --root (default: fb-node, the napi-rs addon Electron loads, plus
    the five fb-plugins/*), across ALL target platforms, so Windows/macOS-only
    crates are listed too. Build-only crates (build scripts, proc-macros) are
    kept and marked, because some of them (e.g. sqlcipher-src, openssl-src)
    compile C code into the binary.
  * npm: every non-dev package in electron/package-lock.json (production
    dependencies), plus the Electron runtime itself. Electron/Chromium ship
    their own LICENSE and LICENSES.chromium.html, which the installers and
    the Flatpak must keep next to the binary.

Usage (from the repo root; needs `cargo` on PATH, and `npm ci` already run in
electron/ so license files can be read from node_modules):

    python3 tools/gen_third_party_notices.py            # writes THIRD-PARTY-NOTICES.md
    python3 tools/gen_third_party_notices.py --check    # exit 1 if file is stale

Run it at every release after updating dependencies. Uses only the Python
standard library.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from collections import defaultdict
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
DEFAULT_ROOTS = [
    "fb-node",
    "fb-plugin-openai",
    "fb-plugin-simplefin",
    "fb-plugin-smtp",
    "fb-plugin-gmail",
    "fb-plugin-m365",
]
# Crates that vendor C sources whose own license files live below the crate
# root; read them too so the copyright of the embedded code is reported.
EXTRA_LICENSE_FILES = {
    "sqlcipher-src": ["sqlcipher/LICENSE-sqlcipher", "sqlcipher/LICENSE-libtomcrypt"],
    "openssl-src": ["openssl/LICENSE.txt"],
}
LICENSE_FILE_RE = re.compile(r"^(LICEN[CS]E|COPYING|COPYRIGHT|NOTICE|UNLICENSE)", re.I)
# A real copyright line, not the Apache appendix template or prose about notices.
COPYRIGHT_RE = re.compile(r"^\s*(?:[*#/;-]+\s*)?(copyright\s*(?:\(c\)|©|\d{4}).*|\(c\)\s*\d{4}.*|©\s*\d{4}.*)$", re.I)
COPYRIGHT_SKIP = re.compile(r"\[yyyy\]|\{yyyy\}|<year>|\[name of copyright owner\]|copyright notice|copyright holders? (and|or)|copyright license", re.I)

# Heuristics for picking one representative text per SPDX id (filename, content).
LICENSE_TEXT_HINTS = {
    "MIT": (r"MIT", r"Permission is hereby granted, free of charge"),
    "Apache-2.0": (r"APACHE", r"Apache License\s+Version 2\.0"),
    "Apache-2.0 WITH LLVM-exception": (r"", r"LLVM Exceptions to the Apache 2\.0 License"),
    "BSD-2-Clause": (r"BSD", r"Redistributions in binary form must reproduce"),
    "BSD-3-Clause": (r"BSD|LICEN", r"Neither the name"),
    "ISC": (r"", r"Permission to use, copy, modify, and(/or)? distribute this software for any"),
    "0BSD": (r"0BSD|LICEN", r"Permission to use, copy, modify, and/or distribute this software for any"),
    "Zlib": (r"ZLIB|LICEN", r"This software is provided .as-is., without any express or implied"),
    "BSL-1.0": (r"BOOST|BSL|LICEN", r"Boost Software License"),
    "Unlicense": (r"UNLICENSE", r"This is free and unencumbered software"),
    "Unicode-3.0": (r"", r"UNICODE LICENSE V3"),
    "CDLA-Permissive-2.0": (r"", r"Community Data License Agreement"),
    "WTFPL": (r"tomcrypt|WTFPL", r"DO WHAT THE FUCK YOU WANT|public domain"),
}


def run(cmd: list[str], cwd: Path) -> str:
    return subprocess.run(cmd, cwd=cwd, check=True, capture_output=True, text=True).stdout


def norm_expr(expr: str | None) -> str:
    if not expr:
        return "NOASSERTION"
    e = expr.strip()
    e = re.sub(r"\s*/\s*", " OR ", e)  # deprecated "MIT/Apache-2.0"
    # Canonical ordering of a pure OR list so "MIT OR Apache-2.0" == "Apache-2.0 OR MIT".
    if " AND " not in e and "(" not in e and " WITH " not in e:
        e = " OR ".join(sorted(p.strip() for p in e.split(" OR ")))
    return e


def license_files(pkg_dir: Path) -> list[Path]:
    out = []
    try:
        for p in sorted(pkg_dir.iterdir()):
            if p.is_file() and LICENSE_FILE_RE.match(p.name):
                out.append(p)
    except OSError:
        pass
    return out


def copyright_lines(files: list[Path], limit: int = 4) -> list[str]:
    seen: list[str] = []
    for f in files:
        try:
            text = f.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        for line in text.splitlines()[:400]:
            m = COPYRIGHT_RE.match(line)
            if not m:
                continue
            c = re.sub(r"\s+", " ", m.group(1)).strip().rstrip(".")
            if COPYRIGHT_SKIP.search(c) or len(c) > 200:
                continue
            if c not in seen:
                seen.append(c)
            if len(seen) >= limit:
                return seen
    return seen


def md_escape(s: str) -> str:
    return s.replace("|", "\\|").replace("<", "&lt;").replace(">", "&gt;")


# --------------------------------------------------------------------------- Rust
def rust_packages(roots: list[str]) -> list[dict]:
    meta = json.loads(run(["cargo", "metadata", "--format-version", "1"], REPO))
    pkgs = {p["id"]: p for p in meta["packages"]}
    by_name = {}
    for p in meta["packages"]:
        if p["source"] is None:
            by_name[p["name"]] = p["id"]
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    missing = [r for r in roots if r not in by_name]
    if missing:
        sys.exit(f"unknown workspace crate(s): {missing}")

    # Walk normal+build edges. A crate is "linked" if reachable through normal
    # edges only (and is not a proc-macro); otherwise it is build-time only.
    linked: set[str] = set()
    anyreach: set[str] = set()

    def walk(start: list[str], normal_only: bool, acc: set[str]) -> None:
        stack = list(start)
        while stack:
            pid = stack.pop()
            if pid in acc:
                continue
            acc.add(pid)
            pkg = pkgs[pid]
            if normal_only and any("proc-macro" in t["kind"] for t in pkg["targets"]):
                continue  # proc-macro deps are compile-time only
            for d in nodes[pid]["deps"]:
                kinds = {k["kind"] for k in d["dep_kinds"]}
                follow = (None in kinds) if normal_only else bool(kinds & {None, "build"})
                if follow:
                    stack.append(d["pkg"])

    root_ids = [by_name[r] for r in roots]
    walk(root_ids, True, linked)
    walk(root_ids, False, anyreach)

    out = []
    for pid in anyreach:
        p = pkgs[pid]
        if p["source"] is None:
            continue  # our own workspace crates
        is_pm = any("proc-macro" in t["kind"] for t in p["targets"])
        usage = "linked" if pid in linked and not is_pm else ("proc-macro" if is_pm else "build-time")
        if p["name"] in EXTRA_LICENSE_FILES:
            usage = "build-time; vendored C linked into binary"
        pdir = Path(p["manifest_path"]).parent
        files = license_files(pdir)
        files += [pdir / x for x in EXTRA_LICENSE_FILES.get(p["name"], []) if (pdir / x).is_file()]
        if p.get("license_file"):
            lf = pdir / p["license_file"]
            if lf.is_file() and lf not in files:
                files.insert(0, lf)
        cr = copyright_lines(files)
        if not cr and p.get("authors"):
            authors = ", ".join(re.sub(r"\s*<[^>]*>", "", a) for a in p["authors"])
            cr = [f"Copyright (c) {authors} (from package authors; no copyright line in license files)"]
        out.append({
            "eco": "crate",
            "name": p["name"],
            "version": p["version"],
            "license": norm_expr(p["license"]),
            "copyright": cr or ["(no copyright line found; see the crate's license files)"],
            "url": p.get("repository") or f"https://crates.io/crates/{p['name']}",
            "usage": usage,
            "files": files,
        })
    return sorted(out, key=lambda x: (x["name"], x["version"]))


# --------------------------------------------------------------------------- npm
def npm_packages() -> tuple[list[dict], dict]:
    eroot = REPO / "electron"
    lock = json.loads((eroot / "package-lock.json").read_text(encoding="utf-8"))
    out = []
    electron = None
    for path, info in lock.get("packages", {}).items():
        if not path:
            continue
        name = path.split("node_modules/")[-1]
        pdir = eroot / path
        pj = {}
        try:
            pj = json.loads((pdir / "package.json").read_text(encoding="utf-8"))
        except (OSError, ValueError):
            pass
        lic = pj.get("license") or info.get("license")
        if isinstance(lic, dict):
            lic = lic.get("type")
        if name == "electron":
            electron = {"version": info.get("version"), "license": lic, "dir": pdir}
        if info.get("dev") or info.get("devOptional"):
            continue
        files = license_files(pdir)
        cr = copyright_lines(files)
        if not cr:
            author = pj.get("author")
            if isinstance(author, dict):
                author = author.get("name")
            if author:
                cr = [f"Copyright (c) {author} (from package.json author)"]
        out.append({
            "eco": "npm",
            "name": name,
            "version": info.get("version", "?"),
            "license": norm_expr(lic),
            "copyright": cr or ["(no copyright line found; see the package's license files)"],
            "url": f"https://www.npmjs.com/package/{name}",
            "usage": "bundled",
            "files": files,
        })
    return sorted(out, key=lambda x: x["name"]), electron or {}


# --------------------------------------------------------------------------- texts
def pick_license_texts(entries: list[dict]) -> dict[str, str]:
    ids: set[str] = set()
    for e in entries:
        ids.update(t for t in re.split(r"\s+(?:OR|AND)\s+|[()]", e["license"]) if t.strip())
    ids = {i.strip() for i in ids}
    # "X WITH Y" stays whole; that's what we want.
    texts: dict[str, str] = {}
    for spdx in sorted(ids):
        hint = LICENSE_TEXT_HINTS.get(spdx)
        if not hint:
            continue
        fname_re, body_re = hint
        for e in entries:
            if spdx not in e["license"]:
                continue
            for f in e["files"]:
                if fname_re and not re.search(fname_re, f.name, re.I):
                    continue
                try:
                    body = f.read_text(encoding="utf-8", errors="replace")
                except OSError:
                    continue
                if re.search(body_re, body, re.I | re.S) and len(body) < 40000:
                    if spdx == "Apache-2.0" and "LLVM Exceptions" in body:
                        continue
                    texts[spdx] = f"(text as distributed with {e['name']} {e['version']})\n\n" + body.strip()
                    break
            if spdx in texts:
                break
    if any("blessing" in e["license"] for e in entries):
        texts["blessing"] = (
            "(SQLite public-domain dedication, as it appears in sqlite3.c / sqlcipher.c)\n\n"
            "The author disclaims copyright to this source code.  In place of\n"
            "a legal notice, here is a blessing:\n\n"
            "   May you do good and not evil.\n"
            "   May you find forgiveness for yourself and forgive others.\n"
            "   May you share freely, never taking more than you give."
        )
    return dict(sorted(texts.items()))


# --------------------------------------------------------------------------- render
def render(crates: list[dict], npm: list[dict], electron: dict, roots: list[str]) -> str:
    L: list[str] = []
    w = L.append
    w("# Third-Party Notices")
    w("")
    w("FreedomBooks is licensed under AGPL-3.0-or-later. It includes the third-party")
    w("components listed below, each under its own license. This file is generated by")
    w("`tools/gen_third_party_notices.py`; do not edit it by hand — regenerate it for")
    w("every release.")
    w("")
    w("License expressions are SPDX as declared by each package (`A OR B` means you may")
    w("use either; `A AND B` means both apply). Copyright lines are taken from each")
    w("package's license/notice files; where none was present the package authors are")
    w("given instead and marked as such.")
    w("")
    w("## Electron and Chromium")
    w("")
    ev = electron.get("version", "?")
    w(f"The desktop app runs on **Electron {ev}** (MIT, Copyright (c) Electron contributors;")
    w("Copyright (c) 2013-2020 GitHub Inc.), which embeds **Chromium**, **Node.js** and")
    w("several hundred further components (BSD-3-Clause, MIT, Apache-2.0, LGPL-2.1, MPL,")
    w("and others). Electron distributes their notices itself in two files that sit next")
    w("to the Electron executable:")
    w("")
    w("* `LICENSE` — Electron's own MIT license")
    w("* `LICENSES.chromium.html` — license texts for Chromium, Node.js, V8, FFmpeg and")
    w("  every other component compiled into the Electron binary")
    w("")
    w("Both files must ship unmodified in every FreedomBooks installer and in the Flatpak.")
    w("They are not reproduced here.")
    w("")
    groups: dict[str, list[dict]] = defaultdict(list)
    for e in crates + npm:
        groups[e["license"]].append(e)
    n_linked = sum(1 for c in crates if c["usage"] == "linked")
    w("## Summary")
    w("")
    w(f"* Rust crates: **{len(crates)}** ({n_linked} linked into the binary, "
      f"{len(crates) - n_linked} build-time only: build scripts / proc-macros)")
    w(f"  — dependency closure of `{', '.join(roots)}` across all target platforms")
    w(f"* npm production packages bundled with the app: **{len(npm)}**"
      + (" (the app has no npm runtime dependencies; `electron` and `playwright` are"
         " devDependencies — Electron's runtime is covered above)" if not npm else ""))
    w("")
    w("| License expression | Packages |")
    w("|---|---:|")
    for lic in sorted(groups, key=lambda k: (-len(groups[k]), k)):
        w(f"| `{md_escape(lic)}` | {len(groups[lic])} |")
    w("")
    w("Usage column: **linked** = compiled into the shipped binary; **build-time** = runs")
    w("only during the build (a build-script dependency; note `sqlcipher-src` and")
    w("`openssl-src` compile bundled C sources — SQLCipher/SQLite/libtomcrypt and")
    w("OpenSSL 3 — into the binary); **proc-macro** = compile-time code generator.")
    w("")
    w("## Components by license")
    for lic in sorted(groups, key=lambda k: (-len(groups[k]), k)):
        w("")
        w(f"### {md_escape(lic)}")
        w("")
        w("| Package | Version | Type | Usage | Copyright |")
        w("|---|---|---|---|---|")
        for e in sorted(groups[lic], key=lambda x: (x["name"], x["version"])):
            cr = "<br>".join(md_escape(c) for c in e["copyright"])
            w(f"| [{md_escape(e['name'])}]({e['url']}) | {e['version']} | {e['eco']} | {e['usage']} | {cr} |")
    texts = pick_license_texts(crates + npm)
    if texts:
        w("")
        w("## License texts")
        w("")
        w("One copy of each license text. For licenses that embed a copyright line (MIT,")
        w("BSD, ISC, ...), the text applies to each package above with that package's own")
        w("copyright line substituted.")
        for spdx, body in texts.items():
            w("")
            w(f"### {spdx}")
            w("")
            w("```text")
            w(body.replace("```", "``​`"))
            w("```")
    w("")
    return "\n".join(L)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--root", action="append", help="workspace crate that ships (repeatable)")
    ap.add_argument("-o", "--output", default=str(REPO / "THIRD-PARTY-NOTICES.md"))
    ap.add_argument("--check", action="store_true", help="fail if the output file is out of date")
    a = ap.parse_args()
    roots = a.root or DEFAULT_ROOTS
    crates = rust_packages(roots)
    npm, electron = npm_packages()
    text = render(crates, npm, electron, roots)
    out = Path(a.output)
    if a.check:
        cur = out.read_text(encoding="utf-8") if out.exists() else ""
        if cur != text:
            print(f"{out} is out of date; run tools/gen_third_party_notices.py", file=sys.stderr)
            return 1
        return 0
    out.write_text(text, encoding="utf-8")
    print(f"wrote {out} ({len(crates)} crates, {len(npm)} npm packages)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
