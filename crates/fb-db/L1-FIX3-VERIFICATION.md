# L1-FIX3 verification (base 15a0919)

## Changed

- New migration 0004_permanent_identities adds BEFORE UPDATE OF guards for the
  primary keys of all nine protected tables (six UUID keys and all components of
  the three composite keys), even for unused operators/accounts and draft lines.
- Account company_id, line journal_id and journal reversal_of_id are permanent
  from creation. Existing journal ownership/retry guards and append-only guards
  protect the other identities; draft account/position edits and posting remain valid.
- Registered 0004 in MIGRATIONS and all twelve triggers in REQUIRED_TRIGGERS.
  Versioned definition hashes automatically include the new definitions while
  preserving the exact v1-v3 definitions. Migrations 0001-0003 are unchanged.
- Added the primary-key rule, complete identity inventory, module test matrix,
  downstream migration-order instructions and native UDF/collation trust contract
  to GUARD-PATTERNS.md.
- Adapted two reversal tests to set their origin at INSERT, preserving the original
  exact-line and inherited-adjusting-flag posting assertions. Message coverage
  accepts the permanent-key refusal for three subsumed old PK-only messages;
  all original refusing statements remain exercised.

## Verified (own command output)

- Failing first: the final four identity tests ran against 15a0919's original
  fb-db source. Three failed (operator rekey/reinsert, all-table fresh-key matrix,
  foreign ownership/origin inventory); the legitimate-edit test passed. The final
  matrix isolates one source row, avoiding sibling UNIQUE collisions as false positives.
- `cargo test --workspace`: 128 passed, 0 failed (119 runtime tests,
  9 compile-fail doctests).
- `cargo clippy -p fb-db -p fb-core -p fb-money -p fb-ids -p fb-harness --all-targets -- -D warnings`:
  exit 0, clean.
- `FB_HARNESS_TEMPLATE=fbv2_l1_f3ref FB_HARNESS_DB=fbv2_l1_f3 FB_HARNESS_OUT=.../harness crates/fb-harness/run_harness.sh`:
  exit 0, 8/8 MATCH; 2,746 audit events compared; 3 known
  collation warnings in edge-multi-company.
- Every primary-key component is tested with ordinary UPDATE and UPDATE OR
  REPLACE, with recursive_triggers ON/OFF, immediate statement refusal and full
  unchanged snapshots of all nine tables. The operator callback test also checks
  actor resolution, membership access, check() and exact snapshots after reopening.
- Populated v2-to-v4 (including the existing copy migration) and direct v3-to-v4
  upgrade tests preserve posted journals, reversals, lines, operators, memberships,
  audits and seals row-for-row; both reopen successfully.
- All twelve new trigger definitions are independently dropped and replaced with
  same-name no-op triggers; check() and reopening refuse every changed schema.
- `git diff --check`: clean. Dropped this job's two verification databases and
  confirmed their exact names are absent from pg_database.
- Preview URL/screenshots: not applicable; Rust database change with no UI/server.

Evidence: `/mnt/Data/Projects/FreedomBooks/runs/2026-10-30/l1-fix3-evidence/`:
`red-final-tests-on-base.txt`, `workspace-final.txt`, `clippy-final.txt`,
`harness-final.txt`, scenario dumps/comparisons under `harness/`,
`database-cleanup.txt` and `summary.json`.

## Left

Lead review and cherry-pick/merge only. No merge, publication, downstream-worktree
change or Cargo.lock commit. Downstream migrations occupying 0004 must be renumbered
after this L1 migration and new protected tables must copy the permanent-key rule.

## Suggested skill lessons

Test fresh-key moves followed by reinsertion alongside replacement collisions and
DELETE/reinsert. Isolate one source row so a sibling UNIQUE conflict cannot mask a
missing guard. Inventory ownership and foreign identities explicitly, preserve
legitimate draft edits, and test both populated upgrade paths and definition hashes.
Native UDF/collation trust needs its own contract beyond SQL authorization.
