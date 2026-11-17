# Original lane books

These encrypted synthetic books were written by the unmodified product source from the exact commits in `manifest.json`. Each source tree was extracted with `git archive`; only the test helper was appended to its existing unit-test module. The current writer did not construct, relabel, or rewrite either fixture.

Append `purchasing-writer.rs` to `crates/fb-core/src/payables/tests.rs` in the purchasing source, and `tax-writer.rs` to `crates/fb-core/src/salestax_payments_tests.rs` in the tax source. Run:

    FB_LANE_FIXTURE_OUT=<new absolute synthetic book path> cargo test -p fb-core --lib write_retained_lane_fixture

The purchasing book contains a posted supplier bill and partial payment; its raw SQLCipher key is byte 42 repeated 32 times. The tax book contains a posted taxed invoice and partial agency payment; its key is byte 31 repeated 32 times. All data and credentials are synthetic. `manifest.json` pins the source migration names and SQL bodies, including the original lane ordinals.

The compatibility regression authenticates each book's complete old schema, verifies a pre-upgrade snapshot, preserves every old cell and audit row, upgrades to the same schema as a fresh book, and reopens it. It also refuses a tampered old schema before migration.
