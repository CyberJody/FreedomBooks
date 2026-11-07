# CORE-L3b verification

This worker completed the in-progress local merge of aaa964f into core-l3
(first parent 366cc96). Independent review and any merge to main remain the lead's
work. No other worktree, production resource, Python source or other project was
modified. Cargo.lock remains untracked and is excluded from the commit.

## Changed

- Reviewed the resolved fb-db registry union. A script verifies the exact union
  of both parents' required triggers, without duplicates (149 final entries),
  and all numbered migrations. Both callback families, open-time checks and
  strict sales/payables dump validation remain registered.
- Imported core 0007 permanent L1 keys, 0008 permanent L2a keys/ownership, and
  0009 plus code for strict validation, audit rollback, SQL defaults, unused
  reclassification, checked revisions, validated helpers and independent Rust
  ordering. All 20 preexisting SQL files are byte-for-byte unchanged.
- Added 0015_sales_core_adoption and 0026_payables_core_adoption. Final chain:
  core 0001–0009, sales 0010–0015, payables 0020–0026; **user_version 22**.
  New draft customer/vendor/item selections require active directory parents in
  SQL, matching services; historical retained selections and VOID copies remain
  usable. Root identity guards use IS NOT so id=id may accompany a valid revision
  advance, while rekeying and every ownership/collision rule remain protected.
- Sales rejects malformed optional invoice IDs before choosing create/edit.
  All six invoice/bill save/post/void increments use documents::next_revision
  before changing posting snapshots/ledger evidence. Both L3 wrappers already
  include changes and audits inside Writer::atomic; existing late-audit tests
  and the caught-composed bill audit test were retained.
- Added ten L3 unit tests, including both service/SQL matrices over all 15 account
  detail types. Added core-l3-hardening, the 33rd differential scenario, for
  archived retention, refused selections, strict optional values, reactivation,
  posting and unused configured-account reclassification.
- Extended the core table inventory for L3; no coverage was removed. Changed the
  payables no-op rejection test to fresh-key rejection and added explicit positive
  no-op identity tests for both mutable roots.

## Populated upgrade fixtures: exact changes

1. Sales previously used hard-coded prefix/user_version 7, which now means a core
   migration. It now locates 0010_sales by name, builds its exact prefix (v10),
   restores its original shared seals/triggers/views, refuses to discard populated
   later tables, and upgrades to v22. Every existing row/key and outstanding 7.00
   is unchanged; reopen passes.
2. Core L2a v6/v7/v8 fixtures previously removed/replaced triggers on a current
   book; that leaves L3 tables behind and cannot represent a prior core schema.
   They now construct the exact encrypted prefix, copy complete populated rows,
   restore exact original definitions, checkpoint/close/copy the file, explicitly
   apply and inspect v9, then Book::open verifies that prefix and upgrades to v22.
   Every core row/key is unchanged at v9, and every final row/key is unchanged at
   v22 and reopen. No populated later table is discarded.
3. Payables and combined populated-copy tests already derive the prefix by the
   0025_payables_core_hardening name. Kept them intact; in the final chain that
   prefix is v20 and upgrades to v22. They preserve every row/key and the combined
   reports and exercise both UDF families under both callback policies.

Old unmerged L3-lineage books require recreation, as authorized in the brief.
No prior schema hash is blessed and no historical migration SQL is rewritten.

## Failing-first evidence

All logs are in /mnt/Data/Projects/FreedomBooks/runs/2026-10-30/.

- core-l3b-red.log: malformed bool invoice ID created a document; invoice and bill
  revision increments panicked on overflow. New regressions then passed after
  typed-ID validation and checked increments (save/post/void for both modules).
- core-l3b-eligibility-red.log and core-l3b-item-red.log: SQL accepted new archived
  customer/vendor/item selections that services refused. New 0015/0026 guards
  make these tests pass without masking the older scope/collision errors.
- core-l3b-noop-red.log / core-l3b-noop-green.log: both mutable roots initially
  refused unchanged keys during a legitimate revision advance; IS NOT fixes it.
- core-l3b-baseline.log: the old sales version-7 fixture fails on the new registry.
  The rebuilt named-prefix fixture passes in the final workspace run.

## Verified: this worker's command output

- cargo test --workspace: **289 passed, 0 failed,
  1 existing ignored diagnostic**; exit 0. Includes the populated upgrade,
  replacement/ownership, caught-audit, strict helper, tax-vector, corruption and
  independent-order tests. The renamed fresh-rekey test also retains the original
  append-only no-op assertions; it was checked separately by rekey-name-check.log
  (1 passed), followed by another clean required Clippy run.
- cargo clippy -p fb-core -p fb-db -p fb-harness -p fb-money --all-targets -- -D warnings:
  exit 0.
- FB_HARNESS_TEMPLATE=fbv2_core_l3bref FB_HARNESS_DB=fbv2_core_l3b
  crates/fb-harness/run_harness.sh: **33/33 MATCH, 0 failures, 0 warnings,
  3 legitimate punctuation ties**. CARGO_TARGET_DIR was absent.
- The combined trial balance exactly equals Python: debit/credit 3500 cents
  (**35.00 = 35.00**), with full account rows, source references and audits compared.
- Metadata union / historical-SQL byte check: passed. git diff --check: passed.
- Both exact job databases removed and absence verified. Preview/screenshots:
  not applicable to this backend integration.

Exact logs, normalized dumps, comparisons and machine-readable evidence:
/mnt/Data/Projects/FreedomBooks/runs/2026-10-30/core-l3b-final/summary.json and metadata.json.

## Left / suggested skill lessons

Lead independent integration review, then merge to main. No deployment/publication
or scenario-day work was started. Derive migration fixtures from names, reproduce
exact prefix definitions, inspect intermediate populated copies, and test service
eligibility beside SQL for every detail type. Reject malformed optional IDs before
branching; use checked revisions; distinguish unchanged-key assignments from
rekeying. See REBASE-L3.md for the completed downstream adoption checklist.
