# L3b prepayments checkpoint

Branch `l3b-prepayments`, based on `fdd2fa6`. Stop before merge.

Review FIX1 on top of `f7f21d2` is documented in [REVIEW-FIX1.md](REVIEW-FIX1.md).
The original checkpoint counts below predate those fixes.

## Implemented

The frozen `prepayments/services.py`, `application_services.py`, `refund_services.py`,
models, and balance queries are translated for customer deposits without estimate
links. Deposits use the existing receipt root (`CDP`) and a Customer deposits
liability; applications (`CDA`) debit that frozen liability and credit the invoice's
original receivables account; refunds (`CDR`) debit the liability and credit a
designated bank. All three have immutable reversal history, exact request retries,
review freshness checks, scoped authorization, and atomic document/audit writes.
There is no vendor-prepayment workflow in the frozen prepayments module.

Migrations 0070 and 0071 introduce the protected tables, receipt-kind rebuild,
complete-owner obligations, exact posting/evidence checks, settlement bounds,
release dates, and refund/payment check-number collision guards. Prior migration
bytes are unchanged. Pure UDFs are registered on create/open and under both
callback policies. Integrity checks cover typed evidence, historical links,
missing details/seals, journal splits, and balance bounds.

`oracle-guards.txt` contains twelve actual `pg_proc.prosrc` bodies from the migrated
dedicated PostgreSQL reference copy. `read_oracle.py` records how they were read.

## Verification coverage

The module's 30 tests cover service workflows, 1,200 less 500 equals 700, the
accounting equation at each lifecycle step, exact retries after release/void,
stale review/audit rollback, malformed fields, eligibility, independent raw-SQL
guards, release-date ordering, all new table keys and replacement attacks with
recursive triggers on/off, hidden rowids, concurrent application/refund draining, application/receipt/void races,
two competing applications, identical concurrent retries,
open-time corruption detection, and populated-copy upgrades/reopens.

Failing-first regressions captured locally:

- Stored application JSON could disagree with the amount column after offline
  guard removal/restoration; the integrity query now rejects it on check/open.
- A nonnumeric bill-payment CHECK reference was incorrectly reserved by the new
  deposit-refund guard; migration 0071 limits reservations to 1–20 ASCII digits.
- Raw refund insertion could store a formatting character rejected by the service;
  the pure fields predicate now enforces the same check-number text rules.
- Offline edits followed by guard restoration could hide colliding live refund
  check numbers; check/open now rejects those collisions. The integrity scan also
  checks journal ownership, seal obligations, and invoice release-date ordering.

The harness adds strict typed dumps and audit normalization, two adversarial
comparer tests, seven scenarios, and a deterministic random corpus of 500 commands
after 13 setup commands. Existing scenarios remain included. The generator lives
in `crates/fb-harness/py/gen_prepayments_scenarios.py`.

Commands and complete outputs are retained in the worktree's `target/` directory:
`prepayments-final.log`, `workspace-final.log`, `clippy-final.log`,
`full-harness-open-check.log`, `workspace-excluding-owner-fixtures-final.log`,
and the `failing-first-*.log` files. The worker report
records final counts and any subsequent rerun logs. This is core-only work:
no preview URL or screenshots apply.

Final observed results (2026-10-30):

- `cargo test -p fb-core prepayments::`: 30 passed, zero failed.
- `cargo clippy -p fb-core -p fb-db -p fb-harness -p fb-money --all-targets -- -D warnings`: clean.
- Full differential harness: 40/40 MATCH, zero failures, zero warnings, three
  legitimate punctuation collation ties. The prepayment random run has 513 total
  commands (13 setup + 500 random), 33 journals, 66 lines, and 161 audit events.
  It took Python 19.6 seconds and Rust 1.9 seconds on the shared machine.
- Workspace with only the four owner upgrade fixtures below excluded: 317 passed,
  zero failed, one existing ignored test. The unfiltered workspace gate is **not
  green**. Its initial run had five failures; the shared table registry failure
  was fixed by adding this module's seven table names, and the four ownership
  requests below remain. No failing test was weakened or deleted.
- Only `fbv2_l3d_harness` is retained; the reference template was dropped.
- No merge, publication, remote, LICENSE, real data, or unrelated project changes.

## Remaining integration requests (not merge-ready)

1. **Sales / estimates:** the base has no estimate service/tables. Non-null
   `estimate_id` is currently refused; 0070 explicitly constrains it to NULL.
   An integration migration must replace that constraint and guard the same
   company/customer estimate link, while `prepare` must read the current estimate
   number for the review digest and audit. Python's estimate test remains pending.
2. **Banking:** `void_deposit` has the optional `banking_depositline` /
   `banking_depositreversal` lookup hook. Exercise it with banking's public
   make-deposit/reversal services and add the corresponding SQL release guard
   after that module lands. Moving undeposited receipts to bank is not implemented
   by prepayments. The frozen banking integration test remains pending.
3. **Credits / checks:** integrate the shared live check-number namespace in both
   service directions and SQL, and extend `prepayments_live_settlements` with live
   credit application lines. `invoice_open_balance` already reads credit lines if
   their expected tables exist, but those hooks cannot be verified on this base.
4. **Payables:** the numeric collision with a live deposit refund is safely refused
   by `pp_payment_number`, but `payables::review_payment` / `record_payment` need
   the Python service preflight and exact ValidationError text. The current
   record call yields the SQL integrity error. Refund-after-payment uses the
   existing `fb_payables_live_checks` and Python's full refusal wording.
5. **Financial reports:** port the cash-basis application/unapplication event hooks.
   This base only has register/trial-balance reports. The accounting equation is
   tested here; Python's cash-basis income/report grouping test is not translated.
6. **Upgrade fixture owners:** the sales prefix upgrade fixture retains the newer
   receipt CHECK while relabeling the schema as old. Build a real prior-schema copy
   (as the prepayments test does). The payables populated-copy test and the two
   `fb-harness/src/core_l3_tests.rs` copy tests compare snapshots with different
   table sets before upgrade; compare the prior table projection and verify new
   tables are empty. These four fixtures were left unchanged under module ownership.

The refund-versus-check concurrency case remains pending the checks module.
The port exercises real concurrent application/refund, application/receipt,
application/deposit-void, application/invoice-void, two-application, and identical
retry races. Review FIX1 adds actual lock-contention checks in both acquisition orders for
all six available races; see `fix1_both_lock_orders_for_every_available_prepayment_race`.

## Python behavior retained

- A deposit is a liability and is not netted against receivables until applied.
- Review tokens bind to remaining balances and frozen account/display snapshots;
  historical customers may be archived for applications/refunds.
- Exact request retries return original evidence even after later use or reversal.
- Original accounts remain authoritative after accounting setup changes.
- Empty-message `DoesNotExist` outcomes and Python date/UUID/text normalization
  are preserved. Refund numbers use ASCII edge whitespace stripping; bill payment
  references reserve numbers only for ASCII digits.
- Refund voids/unapplications release balances on their own dates; deposit and
  invoice voids cannot precede those releases.

No known Python service bug was intentionally changed. The failing-first fixes
above correct this port or enforce the required SQLCipher tamper boundary; they
are not claimed as Python bug fixes.

## Suggested skill lessons

Use real prior-version schema copies for upgrade tests, then compare the prior
table projection before migration and complete snapshots afterward. Keep the
cross-module check-number namespace and invoice-settlement sources explicit in
integration contracts. Verify randomized corpora create real posted documents,
and compare empty exception messages exactly rather than weakening warnings.
