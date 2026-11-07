# L3b banking review FIX1

New commits on `l3b-banking`, from reviewed `1525ef2`. Stop before merge.
Only banking-owned code, its harness adapter/comparer/scenarios, and the minimum
shared migration registration/dependency declarations changed. Earlier migrations
0050–0053 are unchanged; the new migration is `0054_banking_review`.

| Review finding | Fix | Regression evidence |
| --- | --- | --- |
| 1. Stored JSON types | Shared, pure, table-specific payload predicates enforce required fields, scalar types, snapshot ownership, money, revision and transition shapes. JSON booleans remain distinct from integers. Insert/update guards, seal checks, open-time violation queries and fallible reads use the predicates. Stored JSON display defaults were removed. | Copied numeric-boolean and malformed-deposit tests; independent per-field deposit/reversal/reconciliation corruption tests require typed-read, check and reopen refusal; a separate seal test bypasses insert guards in the test fixture. |
| 2. Dates and keys | Canonical calendar parsing returns false for invalid or NULL dates. Canonical UUID syntax is checked for banking identities, references and retry keys at write/open boundaries. Input dates reuse `py::model_date`, after checking the Python ISO layout. | Copied invalid-calendar and completion-key tests; leap-day/NULL/date-shape predicates in both callback types; independent corrupted stored dates and keys fail reopen. |
| 3. New charge references | New service/interest references require active, postable accounts with the correct company/kind, and an eligible register bank. Eligibility is conditional on the owning reconciliation first introducing the reference. Existing references remain immutable historical evidence. | Copied inactive-expense regression; eight new service/interest cases cover inactive/nonpostable accounts and inactive/undesignated banks; historical charges still complete/reopen after account deactivation. |
| 4. Register filters | Ported form normalization, blank optional fields, currency/comma stripping, positive exact amounts, bounded dates, choices and all presets. `build_on` injects the clock. Manual journals search stored `entry_number`, including reversals. Both harness adapters use the same explicit date. | Copied blank/currency/reversal tests; fixed-clock tests cover all presets and invalid filters. All three reviewer scenarios were copied into the differential suite. |
| 5. Aggregate amounts | Screen debit/credit displays and report-group totals use checked aggregate cents, with checked signs/addition. Document/storage boundaries retain their smaller document bound. | Copied two-maximum-documents screen test, original aggregate register test, and reviewer large-screen differential scenario. |
| 6. Comparer | Validates actual fixed-point cents bounds, dates, UUIDs, positive document/line values, state/revision consistency, snapshot ownership and reconciliation transitions. Reports must have exactly left/right sides, complete unique groups/rows, counts and checked totals/signs. | Identical-corruption self-tests cover the reviewer's five cases plus negative line money, invalid register state and inconsistent report counts. Final binary also rejects all five original corrupted dump files. |

Failing-first logs under `runs/2026-10-30/` use prefix
`l3b-banking-fix1-`: `red-core.log` shows **9 failed, 1 control passed**;
`red-presets.log` shows the additional invalid-preset failure; `red-comparer.log`
shows identical malformed statement evidence incorrectly matching before the fix.

Own final command output:

- `cargo test --workspace --no-fail-fast`: **346 passed, 1 failed, 1 ignored**;
  **54 banking tests passed**, including all 11 reviewer/preset regressions.
  The sole failure is the permitted
  `payables::tests::rebase_tests::populated_payables_copy_migration_preserves_every_row_and_reopens`.
- `cargo test -p fb-core --lib banking::tests::review_boundaries -- --nocapture`:
  **7 passed, 0 failed**.
- `cargo test -p fb-harness --lib`: **7 passed, 0 failed**, including the final
  identical-corruption assertions.
- `cargo clippy -p fb-core -p fb-db -p fb-harness -p fb-money --all-targets -- -D warnings`:
  **exit 0**.
- `git diff --check`: **exit 0**.

Evidence uses prefix `runs/2026-10-30/l3b-banking-fix1-`:
`workspace-final.log`, `boundaries.log`, `comparer-final.log`, `clippy-final.log`,
`harness-final.log`, `recompare-final.log`, and `summary.log`.
The full differential suite has **46/46 MATCH**, including the three
reviewer scenarios, with **0 failures, 0 warnings** and the 3 existing punctuation
ties. The latest release comparer replays all 46 pairs successfully and rejects
all 5 original identical corruptions. Populated pre-0050/pre-0051/actor upgrades
remain covered; the new populated 0053→0054 test preserves every existing row.
Harness databases `fbv2_l3b_f1` and `fbv2_l3b_f1ref` were dropped after verification.

Integration helper names: `banking::date` (strict Python model-date input),
`fb_db::banking_evidence::{calendar_date,canonical_uuid,payload,money}` (pure stored
evidence predicates), and `banking::build_on` (explicit register clock).
`money(value, true)` applies document bounds; `money(value, false)` checks signed
i64 aggregate cents. These can be unified with sibling modules during integration.

Left for the lead: independent re-review and merge; the pre-existing payables
populated-copy fixture failure; the shared reversal/provenance wiring and L2
configured-bank reclassification diagnostic already listed in `REQUESTS.md`.
No sibling module was changed. No UI was changed, so preview/screenshots are N/A.
No real data, provider calls, publication, remote git operation or lockfile commit.

Suggested skill lessons: isolate JSON boolean tests from SQL integer tests; test
insert, seal and reopen separately; distinguish newly introduced references from
historical retention; inject clocks at both differential adapters; test document
and aggregate money boundaries independently; replay identical corruptions through
the final comparer, including report membership/count/totals structure.
