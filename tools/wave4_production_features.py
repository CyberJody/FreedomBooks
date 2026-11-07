"""Verify the shipping native bridge cannot construct test restoration capabilities."""
from pathlib import Path
import os
import subprocess

root = Path(__file__).resolve().parents[1]
out = root.parents[1] / "staging/wave4"
env = dict(os.environ)
env["PATH"] = "/home/jody/.cargo/bin:" + env["PATH"]
result = subprocess.run(["cargo", "tree", "-p", "fb-node", "-e", "normal,features", "--prefix", "none"], cwd=root, env=env, check=True, capture_output=True, text=True)
(out / "production-features.txt").write_text(result.stdout)
assert 'feature "test-support"' not in result.stdout
print("Production fb-node dependency graph: no test-support feature")
