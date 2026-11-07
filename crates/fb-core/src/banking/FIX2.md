# L3b banking review FIX2

New work on `l3b-banking`, from reviewed `1dfeb52`; stop before merge.
Only banking, its harness, and the shared JSON helper/registration changed.
Existing migrations are unchanged. New migration: `0055_banking_unique_json`.

| Gap | Fix | Regression evidence |
| --- | --- | --- |
| Duplicate JSON keys | `fb_db::parse_unique_json` rejects repeated decoded object names recursively, including identical values and escaped aliases. `fb_json_unique_keys` is a deterministic, innocuous SQL predicate registered on every book connection. Banking payload predicates and fallible reads use the same parser. Migration 0055 adds insert/update guards and an open-time violation view for all three banking evidence tables. Existing seal validation uses the shared parser through `fb_banking_payload`. | `fix2_duplicate_json_keys_cannot_write_or_seal` reproduces the conflicting hide-later write and verifies rollback. `fix2_duplicate_json_keys_fail_read_seal_check_and_open` independently corrupts stored root/nested/array evidence and requires all four boundaries to refuse it. Two fb-db tests check recursive parsing, escaped duplicates, independent objects, malformed JSON and the SQL predicate. |
| Comparer completeness | Each dump independently checks completed reconciliation membership against its company's completion audit event. Audited IDs, frozen counts/totals and ledger account/company/posted amounts must agree. Each report side must contain exactly the frozen lines belonging to that debit/credit side, with matching ledger amounts, dates, counts and totals. Historical UNDONE statements retain their frozen membership; legitimate empty statements remain valid. | Five failing-first identical-corruption tests cover all missing reconciliation lines, all missing report rows with reset totals/counts, nonexistent report IDs, changed frozen amounts, and deleted lines with stored totals reset. The final release binary also rejects all eight reviewer corruption files. |
| Register normalization | Django's empty-value handling includes empty arrays/objects for optional fields and choices. Character fields reject NUL before filtering. | Six failing-first core tests and six separate harness scenarios cover payee/number NUL, amount []/objects and start []/objects. Each scenario initially MISMATCHed and now MATCHes. |

Own command output:

- `cargo test --workspace --no-fail-fast`: **362 passed, 1 failed, 1 ignored**.
  **63 banking tests passed**. The only failure is the permitted
  `payables::tests::rebase_tests::populated_payables_copy_migration_preserves_every_row_and_reopens`.
- `cargo clippy -p fb-core -p fb-db -p fb-harness -p fb-money --all-targets -- -D warnings`: **exit 0**.
- `FB_HARNESS_TEMPLATE=fbv2_l3b_f2ref FB_HARNESS_DB=fbv2_l3b_f2 bash crates/fb-harness/run_harness.sh`:
  **52/52 MATCH**, **0 failures, 0 warnings**, three existing punctuation ties.
  No `CARGO_TARGET_DIR` was set.
- `fix2_unique_json_migration_preserves_populated_0054_books`: passed, preserving every table row.
  Existing populated prior-schema migration tests also pass.
- Reviewer duplicate-key reproduction: `Err(Integrity { constraint: Some("banking_evidence"), message: "Invalid or duplicate JSON keys" })`; regression exit 0.
- Reviewer comparer reproductions: all eight exit 1/MISMATCH when each corrupt dump is compared with itself.

Failing-first evidence: `runs/2026-10-30/l3b-banking-fix2-red.log`
shows **13 failures** (eight core, five comparer), and
`runs/2026-10-30/l3b-banking-fix2/harness-red.log` shows **six MISMATCHes**.
Final logs live under `runs/2026-10-30/l3b-banking-fix2/`:
`workspace.log`, `clippy.log`, `harness.log`, `register-final.log`,
`reproductions.log`, and `cleanup.log`.

Left: independent re-review and lead merge; reuse the JSON parser and UDF in all
other modules during integration; the previously documented payables fixture,
reversal/provenance hooks and L2 reclassification diagnostic. No sibling module
was modified. Both dedicated harness databases are dropped after verification.
Preview/screenshots: N/A (no UI changes). Local only; no lockfile commit.

Suggested skill lessons: reject duplicate decoded JSON keys at every evidence
boundary; prove report completeness against audit and ledger evidence even when
both dumps agree; port Django empty-container and NUL handling in form tests.
