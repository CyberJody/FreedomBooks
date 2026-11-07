"""Verify the genuine cc40505 fixture from FIX3, without touching its original."""
from pathlib import Path
import os
import shutil
import subprocess
import sys

root = Path(__file__).resolve().parents[1]
out = root.parents[1] / "staging/wave4/upgrade"
source = root.parents[1] / "staging/dfix3/upgrade/cc40505-books"
assert source.is_dir()
out.mkdir(parents=True, exist_ok=True)
revision = subprocess.check_output(["git", "rev-parse", "--short", "HEAD"], cwd=root, text=True).strip()
fixture = out / ("cc40505-" + revision)
assert not fixture.exists(), "Use a fresh W4 upgrade directory"
fixture.mkdir()
shutil.copy2(source / "original.fbbook", fixture / "legacy.fbbook")
shutil.copy2(source / "fixture.json", fixture / "fixture.json")
env = dict(os.environ)
env["PATH"] = "/home/jody/.cargo/bin:" + env["PATH"]
env["CARGO_BUILD_JOBS"] = "4"
env.pop("CARGO_TARGET_DIR", None)
log = out / "verify.log"
with log.open("w") as stream:
    result = subprocess.run(["cargo", "run", "--quiet", "--profile", "test", "-p", "fb-core", "--example", "dfix3_upgrade", "--", "verify", str(fixture)], cwd=root, env=env, stdout=stream, stderr=subprocess.STDOUT)
print("\n".join(log.read_text().splitlines()[-20:]))
print("W4 cc40505 upgrade exit", result.returncode)
sys.exit(result.returncode)
