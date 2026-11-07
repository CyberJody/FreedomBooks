"""Stop only an explicitly named, obsolete foreground W4 verification process."""
from pathlib import Path
import os
import signal
import sys

root = Path(__file__).resolve().parents[1]
def owned(path):
    return path.is_relative_to(root) or path.is_relative_to(root.parents[1] / "staging/wave4")

for arg in sys.argv[1:]:
    parent = int(arg)
    assert owned(Path(os.readlink(f"/proc/{parent}/cwd")))
    descendants = {parent}
    while True:
        before = len(descendants)
        for entry in Path("/proc").iterdir():
            if not entry.name.isdecimal():
                continue
            try:
                stat = (entry / "stat").read_text().rsplit(")", 1)[1].split()
                if int(stat[1]) in descendants:
                    descendants.add(int(entry.name))
            except (OSError, ValueError):
                pass
        if len(descendants) == before:
            break
    for pid in sorted(descendants, reverse=True):
        try:
            assert owned(Path(os.readlink(f"/proc/{pid}/cwd")))
            os.kill(pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
    print("Cancelled obsolete W4 verification", parent, "processes", len(descendants))
