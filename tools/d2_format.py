"""Format D2's newly authored Rust modules without walking or rewriting sibling code."""
import subprocess

subprocess.run(['rustfmt','--edition','2024','--config','skip_children=true',
    'crates/fb-core/src/first_run.rs','crates/fb-core/src/first_run_words.rs',
    'crates/fb-app/src/screens/d2.rs','crates/fb-app/src/screens/d2/audit.rs',
    'crates/fb-core/tests/first_run.rs','crates/fb-app/tests/d2_flow.rs',
    'crates/fb-harness/tests/d2_first_run_comparer.rs',
    'crates/fb-screendiff/src/normalize/literal.rs'],check=True)
