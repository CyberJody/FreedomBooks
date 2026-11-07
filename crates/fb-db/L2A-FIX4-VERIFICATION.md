# L2a-FIX4 verification — 2026-10-30

Base: `core-int` at `95aa359`, including CORE-INT2. No history rewrite, merge,
downstream worktree changes or Python-reference changes. Schema version is 9;
migrations 0001–0008 remain byte-for-byte unchanged. L3 starts at 0010.

## Findings and regressions

1. **Fixed: malformed configuration.** Setup validates role types/IDs and banks
   before mutation, preserving omission/null semantics. Tax rate/group edit IDs
   distinguish omitted/null creation from invalid edits; agency IDs no longer
   lose their type. The copied `review_malformed_setup_*` and
   `review_malformed_edit_id_*` reproductions pass. Expanded setup/type and tax
   edit-ID matrices compare the complete book on refusal and test omission/null
   separately. Python rejects 31 matching malformed setup inputs unchanged;
   malformed rate/group edit IDs fail and omitted/null IDs create.
2. **Fixed: caught audit errors.** `directory::save_record_tx` now encloses the
   entire mutation and audit in `Writer::atomic`. All other public composable
   L2a mutation helpers already use this boundary. The copied caught-error test
   and expanded matrix test directory create/edit for customer/vendor/item and
   document identify/snapshot/post/reverse, catching each injected audit error
   inside an outer successful `Book::write`. Whole-book contents stay unchanged.
   The reviewer's 17-path propagated-error matrix also passes. Book-taking
   setup/directory/tax commands own their complete `Book::write` boundary.
3. **Fixed: SQL vendor eligibility.** Migration 0009 replaces vendor INSERT and
   changed-reference UPDATE guards with the service's exact 11 eligible detail
   types, active/postable and control-account rules. The copied RECEIVABLE test
   and all-15-DetailType service/INSERT/UPDATE matrix pass. SQL retains unchanged
   stale hints; the edit service still requests replacement/clearing, as Python
   does. There is no new service exception for stale defaults.
4. **Fixed: default-only account kind freeze.** 0009 separates historical kind
   protection from the permanent identity/company guards. Journals and source
   snapshots still freeze kind. Unused vendor/item/setup/bank references alone
   do not. The copied vendor-default reclassification reproduction passes;
   the detail matrix exercises retained defaults after reclassification. An
   independent Python run confirms the unused EXPENSE-to-INCOME change.
5. **Fixed: revision exhaustion.** Setup, rate/group edit, tax activation,
   settings and bulk-step mutations use checked increments. At `i64::MAX` they
   return `ValidationError` without changing the book, including a quick-added
   agency or bulk customer/item writes. Stored MAX remains valid exhausted
   evidence. Migration 0009 adds explicit nonnegative i64 bounds for setup and
   both tax revision tables. The copied MAX reproduction and all-six-path
   whole-book regression pass.
6. **Fixed: public helper panics.** Decimal scale is validated before creating
   zero; member shapes, member kinds, rates and labels are validated, with
   checked group-rate addition. The two copied panic reproductions and a
   48-case property test covering decimal/JSON tax helpers pass, including the
   checked-in minimized failure seed. All 1,200 independent Python tax vectors
   are now repository-local regression data and still match.
7. **Fixed: comparer punctuation exception.** Before oracle order comparison,
   Rust chart/TB/directory/tax/snapshot rows are sorted with the embedded ICU +
   UTF-8 contract within their creation-order groups and must equal the original
   sequence. The copied `a-b`/`ab` inversion reproduction passes. Additional
   self-tests reject identical wrong Python/Rust order and wrong Rust order in
   all five collections, while accepting/counting legitimate Python-only ties.

## Own command output

Evidence directory: `/mnt/Data/Projects/FreedomBooks/runs/2026-10-30/`.

- Failing-first runs on the starting implementation: original core reproductions
  **8 failed, 8 passed**, comparer **1 failed, 1 passed**. The expanded suite run
  against base `95aa359` before restoring fixes had **14 failed, 8 passed** in
  core and **1 failed, 1 passed** in the comparer. Logs:
  `l2a-f4-red.log`, `l2a-f4-harness-red.log`, `l2a-f4-expanded-red.log`.
- `cargo test --workspace`: **206 passed, 0 failed, 1 ignored**, including
  11 compile-fail doctests. The ignored test is the existing exotic collation
  diagnostic. `l2a-f4-workspace-final.log`.
- `cargo clippy -p fb-core -p fb-db -p fb-harness -p fb-money --all-targets -- -D warnings`:
  **exit 0**, no warnings. `l2a-f4-clippy.log`.
- `FB_HARNESS_TEMPLATE=fbv2_l2a_f4ref FB_HARNESS_DB=fbv2_l2a_f4
  FB_HARNESS_OUT=.../l2a-f4/harness bash crates/fb-harness/run_harness.sh`:
  **15/15 MATCH, 0 failures, 0 warnings, 3 legitimate punctuation ties**;
  **3,320 audit events** compared. `l2a-f4-harness.log` and `l2a-f4/harness/`.
- Populated encrypted copies at L2a v6/v7/**v8** upgrade to v9, preserving every
  row/key/column, and reopen. Prior-version trigger definitions are reconstructed
  exactly, including definitions replaced by 0009. Existing populated L1 v2/v3
  history/reversal/member/audit/seal upgrade tests also pass.
- Independent Python oracle: **31 malformed setup inputs rejected unchanged**;
  omission/null defaults, rate/group edit IDs, unused default-only kind change,
  and caught nested directory audit rollback confirmed. All synthetic oracle
  changes were rolled back. `l2a-f4-python-oracle.log`.
- `git diff --check`: clean. Python reference `git status --short`: empty.
  Both requested harness DBs were dropped after verification. No UI preview or
  screenshots apply. Cargo.lock was pre-existing/untracked and is uncommitted.

Machine-readable summary: `l2a-f4/verification-summary.json`.

## Left and lessons

Independent re-review and lead merge; no unresolved review finding is known.
L3 jobs must adopt this additive commit and use migration numbers 0010 onward.
`GUARD-PATTERNS.md` and `REBASE-L3.md` record rejecting malformed values rather
than coercing them, auditing inside `Writer::atomic`, matching SQL/service detail
eligibility, separating identity from historical kind, checked revision limits,
public helper property tests and independent Rust-order validation.
