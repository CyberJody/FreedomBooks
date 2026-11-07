#!/usr/bin/env python3
"""Pick the crates `scripts/gate.sh --quick` tests.

Changed files are those since the merge base with FB_GATE_BASE (default main), plus uncommitted and
untracked files. Each file maps to the local package whose directory holds it (from cargo metadata,
deepest path wins); the plan adds every workspace crate that depends on a changed package, transitively,
through normal, build or dev dependencies, including chains through nonmembers. Files outside every
local package are exempt only for known documentation, Electron and gate files. Shared build
files, declared external build inputs and unclassified files test every workspace crate.

Prints the crate names to stdout, one per line, and the reasons to stderr.
Usage: gate_quick_plan.py [WORKSPACE_DIR]
"""
import json
import os
from pathlib import Path
import re
import subprocess
import sys

# Root files that change how every crate builds.
SHARED_BUILD_FILES = {
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain",
    "rust-toolchain.toml",
    ".cargo/config",
    ".cargo/config.toml",
    "clippy.toml",
    ".clippy.toml",
}


def run(args, cwd, text=True):
    return subprocess.run(args, cwd=cwd, check=True, capture_output=True, text=text).stdout


def changed_files(root, base):
    """Absolute paths changed since merge-base(base, HEAD), staged, unstaged or untracked."""
    top = run(["git", "rev-parse", "--show-toplevel"], root).strip()
    merge_base = run(["git", "merge-base", base, "HEAD"], root).strip()
    names = set()
    for args in (
        ["git", "diff", "--name-only", "--no-renames", "-z", merge_base],
        ["git", "ls-files", "--others", "--exclude-standard", "--full-name", "-z"],
    ):
        out = run(args, top, text=False)
        names.update(os.fsdecode(name) for name in out.split(b"\0") if name)
    return merge_base, sorted(os.path.join(top, n) for n in names)


def workspace(root):
    meta = json.loads(run(["cargo", "metadata", "--format-version", "1", "--offline"], root))
    members = {pkg["id"]: pkg["name"] for pkg in meta["packages"]
               if pkg["id"] in meta["workspace_members"]}
    local = {pkg["id"]: pkg for pkg in meta["packages"] if pkg["source"] is None}
    dirs = {os.path.dirname(pkg["manifest_path"]): key for key, pkg in local.items()}
    dependents = {key: set() for key in local}
    for key, pkg in local.items():
        for dep in pkg["dependencies"]:
            if dep.get("path"):
                target = dirs.get(os.path.normpath(dep["path"]))
                if target is None:
                    raise ValueError(f"local dependency missing from cargo metadata: {dep['path']}")
                if target != key:
                    dependents[target].add(key)
    labels = {key: pkg["name"] for key, pkg in local.items()}
    return meta["workspace_root"], dirs, dependents, members, labels, local


def owner(path, dirs):
    best = None
    for d in dirs:
        if path.startswith(d + os.sep) and (best is None or len(d) > len(best)):
            best = d
    return dirs[best] if best else None


def build_inputs(local, dirs):
    """Read literal rerun declarations; uncertain declarations disable outside-file exemptions."""
    inputs = {}
    uncertain = []
    for pkg in local.values():
        directory = os.path.dirname(pkg["manifest_path"])
        for target in pkg["targets"]:
            if "custom-build" not in target["kind"]:
                continue
            script = os.path.normpath(target["src_path"])
            if owner(script, dirs) is None:
                inputs[script] = "external build script"
            try:
                code = Path(script).read_text()
            except (OSError, UnicodeError):
                uncertain.append(script)
                continue
            declarations = re.findall(r'cargo::?rerun-if-changed=([^"\r\n]*)', code)
            if "rerun-if-changed" in code and not declarations:
                uncertain.append(script)
            for value in declarations:
                # Rust escapes, formatting and constructed paths need a conservative fallback.
                if not value or any(character in value for character in "{}\\"):
                    uncertain.append(script)
                else:
                    inputs[os.path.normpath(os.path.join(directory, value))] = "declared build input"
    return inputs, uncertain


def exempt_outside_file(rel):
    return (rel.startswith(("docs/", "electron/", "scripts/tests/")) or rel.endswith(".md")
            or rel in {"scripts/gate.sh", "scripts/gate_quick_plan.py"})


def plan(root, base):
    merge_base, files = changed_files(root, base)
    ws_root, dirs, dependents, members, labels, local = workspace(root)
    log = [f"gate(quick): changes since {base} (merge base {merge_base[:12]}): {len(files)} file(s)"]
    shared = [f for f in files if os.path.relpath(f, ws_root) in SHARED_BUILD_FILES]
    if shared:
        names = ", ".join(os.path.relpath(f, ws_root) for f in shared)
        log.append(f"gate(quick): {names} changed: testing all {len(members)} workspace crates")
        return sorted(members.values()), log
    inputs, uncertain = build_inputs(local, dirs)
    for path in files:
        rel = os.path.relpath(path, ws_root)
        declared = next((kind for target, kind in inputs.items()
                         if path == target or path.startswith(target + os.sep)), None)
        if declared:
            why = f"{declared} {rel!r} changed"
        elif owner(path, dirs) is not None:
            continue
        elif not exempt_outside_file(rel):
            why = f"unclassified file {rel!r} outside any crate changed"
        elif uncertain:
            scripts = ", ".join(repr(os.path.relpath(script, ws_root)) for script in sorted(set(uncertain)))
            why = f"cannot classify {rel!r}: unresolved build inputs in {scripts}"
        else:
            continue
        log.append(f"gate(quick): {why}: testing all {len(members)} workspace crates")
        return sorted(members.values()), log
    direct = {}
    outside = 0
    for f in files:
        name = owner(f, dirs)
        if name is None:
            outside += 1
        else:
            direct[name] = direct.get(name, 0) + 1
    if outside:
        log.append(f"gate(quick): {outside} file(s) outside any crate: known documentation/Electron/gate files, no tests for them")
    reason = {}
    for name in sorted(direct):
        reason[name] = f"changed ({direct[name]} file(s))"
    queue = sorted(direct)
    while queue:
        name = queue.pop(0)
        for dep in sorted(dependents[name]):
            if dep not in reason:
                reason[dep] = f"depends on {labels[name]}"
                queue.append(dep)
    selected = sorted(key for key in reason if key in members)
    for key in selected:
        log.append(f"gate(quick):   test {members[key]}: {reason[key]}")
    if not selected:
        log.append("gate(quick): no crate changed: clippy and core isolation only")
    return sorted(members[key] for key in selected), log


def main():
    root = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else ".")
    base = os.environ.get("FB_GATE_BASE") or "main"
    try:
        crates, log = plan(root, base)
    except ValueError as e:
        print(f"gate(quick): FAILED: {e}", file=sys.stderr)
        return 2
    except subprocess.CalledProcessError as e:
        print(f"gate(quick): FAILED: {' '.join(e.cmd)}: {e.stderr.strip()}", file=sys.stderr)
        return 2
    for line in log:
        print(line, file=sys.stderr)
    for name in crates:
        print(name)
    return 0


if __name__ == "__main__":
    sys.exit(main())
