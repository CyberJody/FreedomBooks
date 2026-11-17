#!/usr/bin/env python3
"""Tests for scripts/gate.sh --quick and scripts/gate_quick_plan.py on a throwaway git + cargo workspace.

cargo is replaced by a logging stub (metadata goes to the real cargo), so nothing is built.
Run: python3 scripts/tests/test_gate_quick.py
"""
import os
import shutil
import subprocess
import tempfile
import unittest

SCRIPTS = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# a <- b (normal) <- c (dev); d alone; crates/group/e is nested under a non-crate folder.
CRATES = {
    "crates/a": ("a", ""),
    "crates/b": ("b", '[dependencies]\na = { path = "../a" }\n'),
    "crates/c": ("c", '[dev-dependencies]\nb = { path = "../b" }\n'),
    "crates/d": ("d", ""),
    "crates/group/e": ("e", ""),
}

CARGO_STUB = """#!/usr/bin/env bash
echo "cargo $*" >> "$GATE_LOG"
if [ "$1" = test ]; then echo "cargo $* FB_TEST_FULL_CASES=${FB_TEST_FULL_CASES:-unset}" >> "$GATE_LOG.env"; fi
if [ "$1" = test ]; then echo "$TMPDIR" >> "$GATE_LOG.tmp"; [ -d "$TMPDIR" ] || echo MISSING >> "$GATE_LOG.tmp"; fi
if [ "$1" = test ]; then echo "${RUST_TEST_THREADS:-unset}" >> "$GATE_LOG.threads"; fi
if [ "$1" = metadata ]; then exec "$REAL_CARGO" "$@"; fi
if [ "${CARGO_STUB_FAIL:-}" = "$1" ] && { [ "$1" != tree ] || [ "${CARGO_TREE_CRATE:-}" = "$3" ]; }; then
  echo "fixture: cargo $1 failed" >&2
  exit "${CARGO_STUB_FAIL_RC:-73}"
fi
if [ "$1" = tree ] && [ "${CARGO_TREE_CRATE:-}" = "$3" ]; then
  echo "${CARGO_TREE_OUTPUT:-}"
fi
exit "${CARGO_STUB_RC:-0}"
"""

NPM_STUB = """#!/usr/bin/env bash
echo "npm $*" >> "$GATE_LOG"
if [ "${NPM_STUB_FAIL:-}" = 1 ]; then echo "fixture: npm failed" >&2; exit 74; fi
"""

# PostgreSQL answers only when the test says so; the real server state must not change the result.

SCRIPT_STUB = """#!/usr/bin/env bash
echo "$0" | sed "s|^$PWD/||" >> "$GATE_LOG"
"""


def write(path, text, mode=None):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        f.write(text)
    if mode:
        os.chmod(path, mode)


class Fixture:
    def __init__(self, gate_sh=None, crates=CRATES):
        self.dir = tempfile.mkdtemp(prefix="gate-quick-")
        self.root = os.path.join(self.dir, "repo")
        members = ", ".join(f'"{p}"' for p in crates)
        write(f"{self.root}/Cargo.toml", f'[workspace]\nresolver = "2"\nmembers = [{members}]\n')
        write(f"{self.root}/Cargo.lock", "version = 4\n")
        write(f"{self.root}/README.md", "fixture\n")
        write(f"{self.root}/crates/group/README.md", "not a crate\n")
        for path, (name, deps) in crates.items():
            write(f"{self.root}/{path}/Cargo.toml",
                  f'[package]\nname = "{name}"\nversion = "0.1.0"\nedition = "2021"\n\n{deps}')
            write(f"{self.root}/{path}/src/lib.rs", "")
        for script in ("crates/fb-db/check_json_features.sh",):
            write(f"{self.root}/{script}", SCRIPT_STUB, 0o755)
        os.makedirs(f"{self.root}/scripts")
        shutil.copy(gate_sh or os.path.join(SCRIPTS, "gate.sh"), f"{self.root}/scripts/gate.sh")
        shutil.copy(os.path.join(SCRIPTS, "gate_quick_plan.py"), f"{self.root}/scripts/gate_quick_plan.py")
        write(f"{self.root}/.gitignore", "/target\n")
        bin_dir = os.path.join(self.dir, "bin")
        write(f"{bin_dir}/cargo", CARGO_STUB, 0o755)
        write(f"{bin_dir}/npm", NPM_STUB, 0o755)
        self.log = os.path.join(self.dir, "gate.log")
        self.env = dict(os.environ)
        self.env.update({
            "PATH": bin_dir + os.pathsep + os.environ["PATH"],
            "REAL_CARGO": shutil.which("cargo"),
            "GATE_LOG": self.log,
            "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@example.invalid",
            "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@example.invalid",
        })
        self.env.pop("FB_GATE_BASE", None)
        self.refresh_lock()
        self.git("init", "-q", "-b", "main")
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "base")
        self.git("checkout", "-q", "-b", "work")

    def refresh_lock(self):
        subprocess.run([self.env["REAL_CARGO"], "generate-lockfile", "--offline"],
                       cwd=self.root, env=self.env, check=True, capture_output=True)

    def git(self, *args):
        subprocess.run(["git", *args], cwd=self.root, env=self.env, check=True)

    def edit(self, rel, text="// changed\n"):
        with open(os.path.join(self.root, rel), "a") as f:
            f.write(text)

    def commit(self):
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "change")

    def plan(self):
        r = subprocess.run(["python3", "scripts/gate_quick_plan.py", "."], cwd=self.root, env=self.env,
                           capture_output=True, text=True, check=True)
        return r.stdout.split(), r.stderr

    def gate(self, *args, cargo_rc=0, **overrides):
        if os.path.exists(self.log):
            os.remove(self.log)
        env = dict(self.env, CARGO_STUB_RC=str(cargo_rc), **overrides)
        r = subprocess.run(["scripts/gate.sh", *args], cwd=self.root, env=env, capture_output=True, text=True)
        commands = []
        if os.path.exists(self.log):
            with open(self.log) as f:
                commands = [c for c in f.read().splitlines() if not c.startswith("cargo metadata")]
        return r, commands

    def close(self):
        shutil.rmtree(self.dir)


CLIPPY = "cargo clippy --workspace --all-targets -- -D warnings"
ISOLATION = [
    "cargo tree -p fb-core -e normal --prefix none",
    "cargo tree -p fb-app -e normal,build --prefix none",
    "cargo tree -p fb-node -e normal,build --prefix none",
    "cargo tree -p fb-remote-proto -e normal,build --prefix none",
    "cargo tree -p fb-host -e normal,build --prefix none",
    "cargo tree -p fb-remote -e normal,build --prefix none",
]
KEY_TESTS = ("cargo test -p fb-app --features key-test-hooks --test key_crashes --test protection_audit "
             "--test protection_publication --no-fail-fast")
ELECTRON = "npm --prefix electron test"
WORKSPACE_TESTS = "cargo test --workspace --no-fail-fast"
FULL_GATE = [CLIPPY, WORKSPACE_TESTS, "crates/fb-db/check_json_features.sh", KEY_TESTS, ELECTRON, *ISOLATION]


class QuickGateTest(unittest.TestCase):
    def setUp(self):
        self.fx = Fixture()

    def tearDown(self):
        self.fx.close()

    def test_no_change_runs_only_clippy_and_isolation(self):
        self.assertEqual(self.fx.plan()[0], [])
        r, commands = self.fx.gate("--quick")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(commands, [CLIPPY, ELECTRON, *ISOLATION])
        self.assertIn("gate(quick): 0 crates tested, clippy clean, Electron unit tests and core isolation OK — passed", r.stdout)
        self.assertIn("no crate changed", r.stderr)

    def test_one_crate_tests_it_and_its_dependents(self):
        self.fx.edit("crates/a/src/lib.rs")
        self.fx.commit()
        crates, why = self.fx.plan()
        self.assertEqual(crates, ["a", "b", "c"])
        self.assertIn("test a: changed (1 file(s))", why)
        self.assertIn("test b: depends on a", why)
        self.assertIn("test c: depends on b", why)
        r, commands = self.fx.gate("--quick")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(commands, [CLIPPY, "cargo test -p a -p b -p c --no-fail-fast", ELECTRON, *ISOLATION])
        self.assertIn("gate(quick): 3 crates tested", r.stdout)

    def test_leaf_crate_tests_only_itself(self):
        self.fx.edit("crates/c/src/lib.rs")
        self.assertEqual(self.fx.plan()[0], ["c"])

    def test_nested_crate_maps_by_path(self):
        self.fx.edit("crates/group/e/src/lib.rs")
        self.assertEqual(self.fx.plan()[0], ["e"])

    def test_untracked_and_uncommitted_files_count(self):
        self.fx.edit("crates/b/src/lib.rs")  # unstaged
        write(os.path.join(self.fx.root, "crates/d/src/extra.rs"), "")  # untracked
        self.assertEqual(self.fx.plan()[0], ["b", "c", "d"])

    def test_git_filenames_are_not_quoted_or_line_split(self):
        for filename in ("café.rs", "tab\tname.rs", "line\nname.rs", 'quote"name.rs', "back\\slash.rs"):
            rel = f"crates/a/src/{filename}"
            with self.subTest(filename=filename, state="untracked"):
                write(os.path.join(self.fx.root, rel), "")
                self.assertEqual(self.fx.plan()[0], ["a", "b", "c"])
            self.fx.git("add", rel)
            self.fx.git("commit", "-q", "-m", "filename")
            self.fx.git("branch", "-f", "main", "HEAD")
            with self.subTest(filename=filename, state="tracked"):
                self.fx.edit(rel)
                self.assertEqual(self.fx.plan()[0], ["a", "b", "c"])
            self.fx.git("restore", rel)

    def test_cargo_lock_tests_all(self):
        self.fx.edit("Cargo.lock", "# changed\n")
        self.fx.commit()
        crates, why = self.fx.plan()
        self.assertEqual(crates, ["a", "b", "c", "d", "e"])
        self.assertIn("Cargo.lock changed: testing all 5 workspace crates", why)

    def setup_nonmember_chain(self):
        self.fx.edit("Cargo.toml", '\nexclude = ["support/x", "support/y"]\n')
        for name, dependency in (("x", "y"), ("y", "d")):
            dependency_path = "../y" if dependency == "y" else "../../crates/d"
            write(os.path.join(self.fx.root, f"support/{name}/Cargo.toml"),
                  f'[package]\nname = "{name}"\nversion = "0.1.0"\nedition = "2021"\n'
                  f'[build-dependencies]\nrenamed = {{ package = "{dependency}", path = "{dependency_path}" }}\n')
            write(os.path.join(self.fx.root, f"support/{name}/src/lib.rs"), "")
        self.fx.edit("crates/a/Cargo.toml",
                     '\n[dependencies]\nrenamed_x = { package = "x", path = "../../support/x" }\n')
        self.fx.refresh_lock()
        self.fx.commit()
        self.fx.git("branch", "-f", "main", "HEAD")

    def test_nonmember_source_tests_member_dependents(self):
        self.setup_nonmember_chain()
        self.fx.edit("support/x/src/lib.rs")
        self.assertEqual(self.fx.plan()[0], ["a", "b", "c"])
        self.fx.git("restore", "support/x/src/lib.rs")
        self.fx.edit("support/y/src/lib.rs")
        self.assertEqual(self.fx.plan()[0], ["a", "b", "c"])

    def test_member_dependencies_through_nonmembers_are_transitive(self):
        self.setup_nonmember_chain()
        self.fx.edit("crates/d/src/lib.rs")
        self.assertEqual(self.fx.plan()[0], ["a", "b", "c", "d"])

    def test_root_cargo_toml_tests_all(self):
        self.fx.edit("Cargo.toml", "# changed\n")
        self.assertEqual(self.fx.plan()[0], ["a", "b", "c", "d", "e"])

    def test_non_crate_files_test_nothing(self):
        self.fx.edit("README.md")
        self.fx.edit("crates/group/README.md")
        self.fx.edit("scripts/gate.sh", "\n")
        crates, why = self.fx.plan()
        self.assertEqual(crates, [])
        self.assertIn("3 file(s) outside any crate", why)

    def build_input_fixture(self, rel, declaration=None):
        write(os.path.join(self.fx.root, rel), "original\n")
        declaration = declaration or f"cargo:rerun-if-changed=../../{rel}"
        write(os.path.join(self.fx.root, "crates/a/build.rs"),
              f'fn main() {{ println!("{declaration}"); }}\n')
        self.fx.commit()
        self.fx.git("branch", "-f", "main", "HEAD")

    def test_external_declared_build_input_tests_all(self):
        self.build_input_fixture("assets/schema.txt")
        self.fx.edit("assets/schema.txt")
        crates, why = self.fx.plan()
        self.assertEqual(crates, ["a", "b", "c", "d", "e"])
        self.assertIn("build input", why)

    def test_external_build_script_tests_all(self):
        self.fx.edit("crates/a/Cargo.toml", 'build = "../../shared-build.rs"\n')
        write(os.path.join(self.fx.root, "shared-build.rs"), "fn main() {}\n")
        self.fx.commit()
        self.fx.git("branch", "-f", "main", "HEAD")
        self.fx.edit("shared-build.rs")
        self.assertEqual(self.fx.plan()[0], ["a", "b", "c", "d", "e"])

    def test_declared_inputs_override_documentation_exemptions(self):
        for rel in ("docs/schema.md", "electron/schema.md", "scripts/tests/schema.md", "guide.md"):
            with self.subTest(path=rel):
                declaration = "cargo::rerun-if-changed=../../docs" if rel.startswith("docs/") else None
                self.build_input_fixture(rel, declaration)
                self.fx.edit(rel)
                crates, why = self.fx.plan()
                self.assertEqual(crates, ["a", "b", "c", "d", "e"])
                self.assertIn("build input", why)

    def test_dynamic_build_inputs_fall_back_to_all(self):
        write(os.path.join(self.fx.root, "crates/a/build.rs"),
              'fn main() { println!("cargo:rerun-if-changed={}", "../../docs/schema.md"); }\n')
        self.fx.commit()
        self.fx.git("branch", "-f", "main", "HEAD")
        write(os.path.join(self.fx.root, "docs/schema.md"), "changed\n")
        crates, why = self.fx.plan()
        self.assertEqual(crates, ["a", "b", "c", "d", "e"])
        self.assertIn("unresolved build inputs", why)

    def test_unclassified_outside_file_tests_all_with_reason(self):
        for rel in ("assets/untracked.txt", "shared-build.rs", "scripts/package.sh"):
            with self.subTest(path=rel):
                write(os.path.join(self.fx.root, rel), "changed\n")
                crates, why = self.fx.plan()
                self.assertEqual(crates, ["a", "b", "c", "d", "e"])
                self.assertIn("unclassified", why)
                self.assertIn(rel, why)
                os.unlink(os.path.join(self.fx.root, rel))

    def test_known_outside_files_still_test_nothing(self):
        for rel in ("docs/notes.txt", "notes.md", "electron/main.cjs", "scripts/tests/fixture.py",
                    "scripts/gate_quick_plan.py"):
            with self.subTest(path=rel):
                self.fx.edit(rel, "# changed\n") if os.path.exists(os.path.join(self.fx.root, rel)) else write(
                    os.path.join(self.fx.root, rel), "# changed\n")
        self.assertEqual(self.fx.plan()[0], [])

    def test_fb_gate_base_moves_the_merge_base(self):
        self.fx.edit("crates/a/src/lib.rs")
        self.fx.commit()
        self.fx.env["FB_GATE_BASE"] = "HEAD"
        self.assertEqual(self.fx.plan()[0], [])

    def test_failure_exits_like_the_full_gate(self):
        r, _ = self.fx.gate("--quick", cargo_rc=101)
        self.assertEqual(r.returncode, 101)
        self.assertIn("gate(quick): FAILED (exit 101)", r.stderr)

    def test_each_isolation_command_failure_fails_both_gates(self):
        for args in (("--quick",), ()):
            for index, crate in enumerate(("fb-core", "fb-app", "fb-node")):
                with self.subTest(args=args, crate=crate):
                    r, commands = self.fx.gate(*args, CARGO_STUB_FAIL="tree", CARGO_TREE_CRATE=crate)
                    self.assertEqual(r.returncode, 73, r.stderr)
                    prefix = [CLIPPY, ELECTRON] if args else FULL_GATE[:5]
                    self.assertEqual(commands, [*prefix, *ISOLATION[:index + 1]])
                    self.assertNotIn("passed", r.stdout)
                    self.assertIn("fixture: cargo tree failed", r.stderr)
                    if args:
                        self.assertIn("gate(quick): FAILED (exit 73)", r.stderr)

    def test_forbidden_dependencies_fail_each_isolation_check(self):
        for crate, dependency in (("fb-core", "napi"), ("fb-app", "reqwest"), ("fb-node", "hyper")):
            with self.subTest(crate=crate):
                r, _ = self.fx.gate("--quick", CARGO_TREE_CRATE=crate,
                                    CARGO_TREE_OUTPUT=f"{dependency} v0.1.0")
                self.assertEqual(r.returncode, 1, r.stderr)
                self.assertNotIn("passed", r.stdout)

    def test_forbidden_dependency_in_large_tree_is_not_hidden_by_sigpipe(self):
        output = "napi v0.1.0\n" + "ordinary v0.1.0\n" * 6000
        r, _ = self.fx.gate("--quick", CARGO_TREE_CRATE="fb-core", CARGO_TREE_OUTPUT=output)
        self.assertEqual(r.returncode, 1, r.stderr)
        self.assertIn("fb-core depends on", r.stderr)

    def test_selected_crate_test_failure_propagates(self):
        self.fx.edit("crates/a/src/lib.rs")
        r, commands = self.fx.gate("--quick", CARGO_STUB_FAIL="test", CARGO_STUB_FAIL_RC="72")
        self.assertEqual(r.returncode, 72)
        self.assertEqual(commands, [CLIPPY, "cargo test -p a -p b -p c --no-fail-fast"])
        self.assertIn("gate(quick): FAILED (exit 72)", r.stderr)

    def test_bad_base_fails_before_clippy(self):
        self.fx.env["FB_GATE_BASE"] = "no-such-branch"
        r, commands = self.fx.gate("--quick")
        self.assertNotEqual(r.returncode, 0)
        self.assertEqual(commands, [])

    def test_unknown_flag_is_refused(self):
        r = subprocess.run(["scripts/gate.sh", "--fast"], cwd=self.fx.root, env=self.fx.env,
                           capture_output=True, text=True)
        self.assertEqual(r.returncode, 2)

    def test_extra_flags_are_refused_before_cargo(self):
        for flags in (('--quick', '--unknown'), ('--quick', '--quick'), ('', '--unknown')):
            r, commands = self.fx.gate(*flags)
            self.assertEqual(r.returncode, 2)
            self.assertEqual(commands, [])

    def test_full_gate_command_list_is_unchanged(self):
        self.fx.edit("crates/a/src/lib.rs")
        r, commands = self.fx.gate()
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(commands, FULL_GATE)
        self.assertIn("gate: clippy, workspace tests, check_json_features.sh, key-change tests, Electron unit tests "
                      "and core isolation passed", r.stdout)


    def test_full_gate_sets_full_case_counts_and_quick_gate_does_not(self):
        self.fx.edit("crates/a/src/lib.rs")
        env_log = self.fx.log + ".env"
        for args, expected in (((), "FB_TEST_FULL_CASES=1"), (("--quick",), "FB_TEST_FULL_CASES=unset")):
            with self.subTest(args=args):
                if os.path.exists(env_log):
                    os.remove(env_log)
                r, _ = self.fx.gate(*args, FB_TEST_FULL_CASES="")
                self.assertEqual(r.returncode, 0, r.stderr)
                with open(env_log) as f:
                    lines = f.read().splitlines()
                self.assertTrue(lines)
                for line in lines:
                    self.assertTrue(line.endswith(expected), line)

    def test_each_run_gets_a_private_temp_folder_removed_at_the_end(self):
        self.fx.edit("crates/a/src/lib.rs")
        parent = os.path.join(self.fx.dir, "tmp-parent")
        os.makedirs(parent)
        tmp_log = self.fx.log + ".tmp"
        # A failing cargo test still removes the folder.
        for args, rc, fail in (((), 0, ""), (("--quick",), 0, ""), (("--quick",), 72, "test"), ((), 72, "test")):
            with self.subTest(args=args, rc=rc):
                if os.path.exists(tmp_log):
                    os.remove(tmp_log)
                r, _ = self.fx.gate(*args, FB_GATE_TMP_PARENT=parent, CARGO_STUB_FAIL=fail, CARGO_STUB_FAIL_RC="72")
                self.assertEqual(r.returncode, rc, r.stderr)
                with open(tmp_log) as f:
                    seen = f.read().splitlines()
                self.assertTrue(seen)
                self.assertNotIn("MISSING", seen)
                self.assertTrue(all(os.path.dirname(t) == parent and os.path.basename(t).startswith("fb-gate.") for t in seen), seen)
                self.assertEqual(os.listdir(parent), [], "the gate removes its temp folder")

    def test_test_threads_default_to_eight_and_can_be_overridden(self):
        self.fx.edit("crates/a/src/lib.rs")
        threads_log = self.fx.log + ".threads"
        for args in ((), ("--quick",)):
            for given, expected in (("", "8"), ("3", "3")):
                with self.subTest(args=args, given=given):
                    if os.path.exists(threads_log):
                        os.remove(threads_log)
                    env = {"RUST_TEST_THREADS": given} if given else {}
                    saved = self.fx.env.pop("RUST_TEST_THREADS", None)
                    try:
                        r, _ = self.fx.gate(*args, **env)
                    finally:
                        if saved is not None:
                            self.fx.env["RUST_TEST_THREADS"] = saved
                    self.assertEqual(r.returncode, 0, r.stderr)
                    with open(threads_log) as f:
                        self.assertEqual(set(f.read().split()), {expected})

    def test_electron_failure_fails_both_gates(self):
        for args in (("--quick",), ()):
            with self.subTest(args=args):
                r, commands = self.fx.gate(*args, NPM_STUB_FAIL="1")
                self.assertEqual(r.returncode, 74, r.stderr)
                self.assertEqual(commands[-1], ELECTRON)
                self.assertNotIn("passed", r.stdout)

    def test_electron_change_is_checked_by_the_electron_tests(self):
        write(os.path.join(self.fx.root, "electron/main/guards.cjs"), "// changed\n")
        crates, why = self.fx.plan()
        self.assertEqual(crates, [])
        self.assertIn("1 Electron file(s) changed: checked by npm --prefix electron test", why)
        self.assertNotIn("no tests for them", why)
        r, commands = self.fx.gate("--quick")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn(ELECTRON, commands)


class KeyTestsGateTest(unittest.TestCase):
    """fb-app's key-change tests need its key-test-hooks feature, so the plain cargo test never runs them."""

    def setUp(self):
        self.fx = Fixture(crates={**CRATES, "crates/fb-app": ("fb-app", '[dependencies]\na = { path = "../a" }\n')})

    def tearDown(self):
        self.fx.close()

    def test_fb_app_change_runs_the_key_tests(self):
        self.fx.edit("crates/fb-app/src/lib.rs")
        r, commands = self.fx.gate("--quick")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(commands, [CLIPPY, "cargo test -p fb-app --no-fail-fast", KEY_TESTS, ELECTRON, *ISOLATION])

    def test_unrelated_crate_skips_them(self):
        self.fx.edit("crates/d/src/lib.rs")
        r, commands = self.fx.gate("--quick")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertNotIn(KEY_TESTS, commands)

    def test_full_gate_always_runs_them(self):
        r, commands = self.fx.gate()
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn(KEY_TESTS, commands)


if __name__ == "__main__":
    unittest.main(verbosity=2)
