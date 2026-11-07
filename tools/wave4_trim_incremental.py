"""Trim this worktree's reproducible cache only while owning Cargo's build lock."""
from pathlib import Path
import fcntl
import shutil
import time

root = Path(__file__).resolve().parents[1]
cache = root / "target/debug/incremental"
assert cache.resolve() == cache
deadline = time.monotonic() + 50
with (root / "target/debug/.cargo-lock").open("r+") as lock:
    while True:
        try:
            # Cover both Linux file-lock mechanisms; never race an active build.
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            fcntl.lockf(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            break
        except BlockingIOError:
            fcntl.flock(lock, fcntl.LOCK_UN)
            if time.monotonic() >= deadline:
                raise SystemExit("Cargo still building; cache retained. Retry after compilation.")
            time.sleep(1)
    if cache.exists():
        removed = sum(p.stat().st_size for p in cache.rglob("*") if p.is_file())
        shutil.rmtree(cache)
        print("Removed W4 incremental cache under Cargo lock:", round(removed / 1024**3, 2), "GiB")
    else:
        print("No W4 incremental cache to trim")
