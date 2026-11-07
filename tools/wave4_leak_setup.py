"""Give the unchanged R11 generator/driver isolated W4 output and build paths."""
from pathlib import Path
root = Path(__file__).resolve().parents[1]
source = (root / "tools/dfix3_leak.py").read_text()
assert "staging/dfix3/leak" in source
source = source.replace("staging/dfix3/leak", "staging/wave4/leak")
source = source.replace("env=dict(os.environ,RUST_TEST_THREADS='1',CARGO_BUILD_JOBS='3')", "env=dict(os.environ,RUST_TEST_THREADS='1',CARGO_BUILD_JOBS='3',CARGO_TARGET_DIR=str(root/'target'))\nenv['PATH']='/home/jody/.cargo/bin:'+env['PATH']")
(root / "tools/wave4_leak.py").write_text(source)
