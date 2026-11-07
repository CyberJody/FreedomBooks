"""Derive the proven foreground gate driver with W4-only paths and databases."""
from pathlib import Path
root = Path(__file__).resolve().parents[1]
source = (root / "tools/dfix3_gates.py").read_text()
source = source.replace("staging/dfix3", "staging/wave4").replace("'dfix3'", "'wave4'")
source = source.replace("fbv2_int_dfix1_df3ref", "fbv2_int_dfix1_w4ref")
source = source.replace("fbv2_int_dfix1_df3screens", "fbv2_int_dfix1_w4screens")
source = source.replace("fbv2_core_df3ref", "fbv2_core_w4ref").replace("fbv2_core_df3", "fbv2_core_w4")
source = source.replace("f'{NODE}:{XV}/bin:'", "f'/home/jody/.cargo/bin:{NODE}:{XV}/bin:'")
source = source.replace("RUST_TEST_THREADS='8'", "RUST_TEST_THREADS='4'")
(root / "tools/wave4_gates.py").write_text(source)
