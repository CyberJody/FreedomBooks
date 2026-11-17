# Authentic installed-build format-90 fixture

Synthetic only. `format90.fbbackup` was written by the unmodified application and
migrations at installed-build commit
`5a8c4fd9c9b8fc06ee2ee7df810f20efd5e86148`, not by relabeling a newer book.
The only added predecessor source was `writer.rs`, copied into
`crates/fb-core/tests/write_bills_jobs_fixture.rs` as a fixture-generating test.

The encrypted book uses the synthetic key `[49; 32]`. It contains two customers,
a vendor, service items, a supplier bill, and draft, posted and voided invoices.
`fixture.json` retains the exact public invoice responses and identifiers.
The backup archive SHA-256 is
`ac3d05e42560398ddea99a6fbca2e51fb5cbb777e64050947cbb8b84f6c79ddb`.

To regenerate, extract that commit into a throwaway source directory, copy the
writer to the test path above, and run with Rust 1.99.1:

```sh
FB_LEGACY_FIXTURE_OUT=/absolute/new/synthetic-directory \
  cargo test --offline --locked -p fb-core --test write_bills_jobs_fixture
```

Retain `backups/format90.fbbackup` and `fixture.json` from the output. A regenerated
fixture has fresh random identifiers and encrypted bytes; update the digest.
The integration regressions in `bills_jobs_upgrade.rs` exercise direct 90→92
upgrade, verified pre-upgrade snapshot, real 90→92 archive restore, preservation
of all prior rows, ordinary invoice edit/post/void, both integrated features,
92→92 backup/restore, post/void after restore and integrity/reopen.
