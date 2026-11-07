"""Free only intermediates from the already-linked, isolated R11 build."""
from pathlib import Path
import shutil

root = Path(__file__).resolve().parents[1]
evidence = root.parents[1] / "staging/wave4/leak"
assert (evidence / "build.log").exists() and (evidence / "driver.log").exists()
debug = evidence / "source/target/debug"
assert debug.resolve() == debug
removed = 0
for name in ("incremental", "build"):
    folder = debug / name
    if folder.exists():
        removed += sum(p.stat().st_size for p in folder.rglob("*") if p.is_file())
        shutil.rmtree(folder)
for file in (debug / "deps").iterdir():
    if file.suffix in (".rlib", ".rmeta"):
        removed += file.stat().st_size
        file.unlink()
print("Removed completed W4 R11 intermediates:", round(removed / 1024**3, 2), "GiB; executable and evidence retained")
