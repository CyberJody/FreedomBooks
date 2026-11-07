# CORE-INT2 verification — 2026-10-30

Base: `core-int` `efd81c1`. Source L1-FIX3 `457893a` was cherry-picked as
`6cb0be8`, with its migration renamed from 0004 to 0007. No history was rewritten.
Migrations 0001–0006 remain byte-for-byte unchanged. Migration 0008 adds the L2a
guards; schema version is 8. Versioned definition hashes are derived from each
numbered migration prefix, preserving all previous versions' expected hashes.

## Changes

- 0007 protects all primary-key components on the nine L1 tables, plus account
  company, line journal and reversal origin.
- 0008 adds eleven L2a primary-key guards and twelve ownership guards, including
  all company_id columns, tax-member group/member, bank setup/account,
  sequence kind, tax-item kind, frozen snapshot source/account/role and journal
  source_id (including NULL transitions). All use BEFORE UPDATE OF and IS NOT.
- REBASE-L3.md requires permanent keys/ownership on every new module table and
  reserves 0009 onward for L3. Both sides' GUARD-PATTERNS.md edits are retained.
- All 35 new trigger definitions are required; 70 drop/same-name-replacement
  cases require rejection at check and reopen.

## Own command results

Evidence directory outside this worktree:
`/mnt/Data/Projects/FreedomBooks/runs/2026-10-30/`.

1. Before 0008, `cargo test -p fb-core --test l2a_identity_permanence -- --nocapture`
   failed both new tests. Valid tax-member group/member and bank account changes
   were accepted with ordinary/OR REPLACE updates and recursive triggers ON/OFF.
   A tax-member reparent/reinsert committed successfully. See `core-int2-red.log`.
2. Final `cargo test --workspace`: **180 passed, 0 failed, 1 ignored**, including
   11 compile-fail doctests. See `core-int2-workspace-final.log`. The ignored test
   is the existing exotic-collation diagnostic.
3. `cargo clippy -p fb-core -p fb-db -p fb-harness -p fb-money --all-targets -- -D warnings`:
   clean. See `core-int2-clippy.log`.
4. `FB_HARNESS_TEMPLATE=fbv2_l2a_c2ref FB_HARNESS_DB=fbv2_l2a_c2 ... crates/fb-harness/run_harness.sh`:
   **15/15 MATCH, 0 failures, 0 warnings, 3 punctuation ties**, with 3,320 full
   audit events compared. See `core-int2-harness.log` and `core-int2/harness/`.
5. Populated L1 v2-copy and v3 upgrades pass through v8, preserving posted
   history, reversals, users, memberships, audits and seals, then reopen.
   Added populated L2a v6/v7 encrypted-copy upgrades preserve every table's rows,
   keys and columns and reopen at v8. Prior L2a schemas are reconstructed by
   removing only the additive 0007/0008 triggers; Book::open validates their exact
   prior-version definition hashes before migration.
6. New L2a matrix covers every primary key and company column, valid alternate
   link parents, journal source transitions and complete unchanged-book snapshots.
   Vendor rekey/reinsert cannot redirect the recorded tax agency. No-op key
   assignments, ordinary edits and service-style link DELETE/INSERT still work.
7. `git diff --check`: clean. Only fb-core/fb-db files changed. Cargo.lock was
   pre-existing/untracked and remains uncommitted. No UI preview/screenshots apply.

The machine-readable summary is `core-int2/verification-summary.json`.

## Left for the lead

Review and merge; L3 jobs must adopt the new commits and start migrations at
0009. No merge or downstream worktree changes were performed. Old unmerged L2a
lineage books still require recreation, as documented by the earlier integration.

## Suggested skill lessons

Test ownership on mutable link rows separately from primary-key collisions.
Use valid alternate parents, test reparent/reinsert under deferred FKs, and
preserve the legitimate DELETE/INSERT service path. Inventory every key part and
ownership column, and number additive hardening migrations after the combined
integration chain rather than the source branch's migration number.
