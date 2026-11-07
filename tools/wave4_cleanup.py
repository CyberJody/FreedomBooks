"""Drop only the four databases allocated to this completed W4 verification."""
from pathlib import Path
import subprocess
import sys

root = Path(__file__).resolve().parents[1]
out = root.parents[1] / "staging/wave4"
harness_only = sys.argv[1:] == ["harness"]
assert not sys.argv[1:] or harness_only
if not harness_only:
    assert "gate: clippy," in (out / "gate.log").read_text(), "Finish the gate first"
harness = (out / "harness.log").read_text()
assert "database left from the last scenario:" in harness, "Finish the harness first"
assert not any(line.startswith(("MISMATCH", "FAIL")) for line in harness.splitlines())
names = ["fbv2_core_w4", "fbv2_core_w4ref", "fbv2_int_dfix1_w4screens", "fbv2_int_dfix1_w4ref"]
if harness_only:
    names = names[:2]
for name in names:
    subprocess.run(["dropdb", "--if-exists", name], check=True)
query = "SELECT datname FROM pg_database WHERE datname IN (" + ",".join("'" + name + "'" for name in names) + ")"
remaining = subprocess.check_output(["psql", "-X", "-At", "-d", "postgres", "-c", query], text=True).strip()
assert not remaining
message = f"Dropped {len(names)} W4 verification databases; 0 remaining."
(out / ("harness-database-cleanup.txt" if harness_only else "database-cleanup.txt")).write_text(message + "\n")
print(message)
