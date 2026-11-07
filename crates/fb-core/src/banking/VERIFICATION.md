# L3b banking verification

Branch `l3b-banking`, based on `fdd2fa6`; stopped before merge.

**Acceptance is not fully green:** the unfiltered workspace suite has one
untouched payables historical-copy fixture failure. See `REQUESTS.md` for that
fixture, the shared reversal-hook requests and the L2 configured-only account
reclassification/open-check mismatch. No sales/payables implementation or tests
were changed by this worker.

Own command results:

- `cargo test -q -p fb-core --lib banking`: **35 passed, 0 failed**.
- `cargo test -q --workspace --no-fail-fast`: **327 passed, 1 failed,
  1 ignored**. The sole failure is
  `payables::tests::rebase_tests::populated_payables_copy_migration_preserves_every_row_and_reopens`:
  its prior-schema table snapshot includes later empty banking tables.
- `cargo clippy -q -p fb-core -p fb-db -p fb-harness -p fb-money --all-targets -- -D warnings`:
  **exit 0, no output**.
- Isolated full harness: **43/43 MATCH, 0 failures, 0 warnings,
  3 existing punctuation ties**. Includes all 33 inherited cases and 10 banking
  cases; the randomized case contains 500 random commands plus setup/finalization.
- Final banking-only rerun after the checked-sign hardening: **10/10 MATCH,
  0 failures, 0 warnings** (`target/banking-focused-final.log`). Its random case
  measured **Python 4.5s, Rust 0.8s**.
- Randomized banking timing from that full run: **python 4.1s, rust 2.3s**. This includes each
  adapter's run and dump, excluding build/template cloning; concurrent jobs make
  it a noisy local measurement.
- Populated upgrade checks preserve ledger/sales/history/audit/seals and banking
  membership/status/actor evidence, including operator rename before upgrade.
  Tampered prior schemas fail closed. Replacement/UPSERT/view/hidden-rowid/PK
  matrices run with recursive triggers both off and on. Late audit and caught
  composed-writer failures leave exact unchanged book snapshots.
- Two-connection races leave one open reconciliation and prevent a deposit and
  sales-receipt void from both winning.
- Register metadata/split query count is constant when growing from 1 to 21 rows;
  large cumulative balances are not capped at one document's amount limit.
- Live Python PostgreSQL guard bodies were read from migrated synthetic copies;
  final capture: `target/banking-python-guards.txt`.

Logs: `target/banking-tests-final.log`, `target/banking-workspace-final.log`,
`target/banking-clippy-final.log`, `target/banking-all-final.log`, and per-scenario
`target/harness/*.compare.txt`. These are local verification artifacts.

Python oddities and the guarded differences are recorded in `README.md`.
Bank-feed matching/acceptance remains in Python's separate `bankfeeds/` scope.
Preview URL/screenshots: not applicable (no presentation layer was built).

Suggested skill lessons: authorize small sibling historical-fixture adaptations
in module briefs; update the shared comparer fixture whenever its dump grows;
test zero-net omitted reconciliation sets and permanent actors on empty
statements, alongside amount equality and ordinary posted-line references.
