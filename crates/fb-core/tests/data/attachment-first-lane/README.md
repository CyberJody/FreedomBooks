# Authentic first attachment-backup fixture

Generated with the unmodified product source at commit `f125c3ea` using the retained `writer.rs` test helper, Rust 1.99.1:

    FB_LEGACY_FIXTURE_OUT=<empty synthetic folder> cargo test -p fb-core --test write_legacy_fixture -- --nocapture

The helper lived at `crates/fb-core/tests/write_legacy_fixture.rs` in a git-archive extraction of that commit. It used the old attachment, re-wrap, SQLCipher rekey and portable backup implementations. No current writer was used to construct or rewrite the archive.

The synthetic book has two companies with one attachment each. The old re-wrap implementation recorded both companies' wraps in an audit event under the first company. Its master key was then changed, so the historical receipt-key wraps are required to read the attachments. Its manifest uses the old substituted-wrap representation. The backup carries the password-sealed portable key envelope.

All data and credentials are synthetic. Book password: `Synthetic legacy book password`; master key: byte 43 repeated 32 times; receipt key: byte 77 repeated 32 times. `fixture.json` names the expected rows and records the exact historical audit detail.

Archive SHA-256: `452e2a2c5452c110e93c5aaff067152b48a795e72d46aaa832eccc47d2b0acb9`. Preserve this fixture and migrations 0196/0197 unchanged.
