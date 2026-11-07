# Checks and transfers translation

This module translates the frozen Python `checks` and `transfers` services and
their non-view tests from `python-reference-20261030`. It records accounting
evidence only. It does not send payments, print checks, or add reconciliation
state. Bill-payment posting remains owned by `payables`.

## API and ownership

- `checks::{save_check, review_check, record_check, assign_number, void_check}`
  preserve append-only revisions, item and payee snapshots, exact retries and
  optimistic revision checks. `get_check` and `current_checks` authorize reads.
- `checks::{next_number, number_text, refuse_conflict}` provide per-bank numbering.
- `checks::{review_bill_payment, record_bill_payment}` are compatibility entry
  points around `payables::{review_payment, record_payment}`. They add the shared
  number preflight without modifying the payables module. A SQL collision guard
  on payment insertion repeats the rule inside its writer transaction.
- `transfers::{review_transfer, record_transfer, void_transfer, posted_balance}`
  move money between designated banks with an exact two-line posting and an
  append-only void record.
- `save_check_tx` and `record_transfer_tx` require `Writer` and include audits in
  `Writer::atomic`, so callers may catch an error without committing partial work.

The five protected tables are STRICT and WITHOUT ROWID. All primary keys are
permanent. The check root may advance exactly one revision, changing its current
bank/vendor/number/state index only alongside a complete sealed revision. Its
company, document identity and creation digest never change. Every revision,
line, transfer and void is append-only. Account, directory, actor, identity and
journal references also protect their parents against replacement and deletion.

Migrations 0030–0032 register the tables, collision/eligibility/history guards,
deferred owner seals and immediate parent checks. Every definition participates
in the existing schema hash. Open-time checks also inspect document totals,
revision transitions, copied financial evidence, source accounts and exact
journal splits. All decimal calculations use the existing Rust money helpers;
the existing pure `fb_payables_line_valid` callback verifies raw line amounts.

## Reference behavior deliberately preserved

- A draft can retain an inactive or nonpostable line account, while posting still
  refuses it. Archived directory selections require explicit retention; refresh
  requires reactivation. Missing draft line accounts are permitted for correction.
- Payee/item/company snapshots remain historical until the corresponding explicit
  refresh. Record, number and void revisions copy financial fields and lines.
- A print-later check can be recorded without a number. Number assignment creates
  another revision. An exact retry adds no revision, sequence, journal or audit.
- Voiding releases a number for reuse, but suggestions consider historical voided
  checks and reversed payments. Suggestions are hints, not reservations.
- Payment references count as check numbers only for CHECK and 1–20 ASCII digits.
  Other written-check numbers are case-sensitive text; leading zeros are retained.
  A 20-digit numeric maximum falls back to the smallest unused positive number.
- Frozen control-account roles continue to exclude accounts after setup changes;
  a BANK receipt destination alone does not become a control account.
- Transfer balances include every posted journal, including reversals. Exact
  record retries still return the original transfer after it has been voided.

No Python service bug was intentionally corrected. Additional raw-SQL rejection
and immutable-identity rules are the required SQLCipher safety contract, including
immediate missing/cross-company parent refusal with deferred foreign keys.

## Verification map

`tests.rs` covers lifecycle, exact retries, print-later numbering, item rounding,
stale reviews, malformed input, account exclusions and late-audit rollback.
`more_tests.rs` covers directory retention/refresh, frozen controls, tenant
isolation, generic journal refusal, two-connection retries, remaining audit paths,
line/money limits and display queries. `primitive_tests.rs` covers every distinct
new guard message, all 15 account detail types, immediate parent ordering and
cross-module payment collisions. `guards_tests.rs` runs unique-key/replacement,
fresh-key, hidden-rowid, view-propagation and schema/evidence corruption tests.
`parity_tests.rs` covers the supplemental corefx/FX4 legacy-duplicate, numeric
payment-reference and ordered edit/record/number/void/setup outcomes. PostgreSQL
row-lock inspection is replaced by SQLCipher writer serialization; independent
connection tests verify exact-once recording. `upgrade_tests.rs` copies populated books through each named migration prefix,
compares every old row/key and reopens; tampered prior schemas are refused.

The Python migration-reversal implementation test is platform-specific; versioned
prefix/hash and populated-copy tests replace it. No Django views/forms/templates
are translated. Capability doctests reject Reader/raw Connection writes.

The harness adapter compares all five tables, snapshots, full audit leaves,
number suggestions and the existing ledger/reports. Six scenarios cover check
and transfer workflows, payment collisions and 500 seeded random operations.
Strict-comparer tests reject identical malformed dumps and altered audit leaves.

## Integration requests (not applied to other owners' code)

1. Historical-copy fixtures must compare the exact prefix's existing tables and
   assert that any later tables in the current source are empty, then compare the
   complete upgraded snapshot. The current whole-map assertion fails solely
   because five new empty tables do not exist at the old prefix. Affected tests:
   `payables::tests::rebase_tests::populated_payables_copy_migration_preserves_every_row_and_reopens`
   and harness `core_l3_tests::{combined_book_copy_upgrade_preserves_rows_reports_and_both_callback_families,
   open_reports_both_sales_and_payables_integrity_violations_with_intact_schema}`.
   The module's own populated-copy test demonstrates the required comparison.
2. Payables should call `checks::refuse_conflict` in its shared-number preflight
   under the writer, or route clients through this module's compatibility entry
   points. Direct payables recording is already protected by the new SQL guard;
   direct review currently lacks the friendly written-check collision message.
3. Credits and prepayments owners must add their refund tables to `next_number`
   and `refuse_conflict`, including self-exclusions and symmetric SQL collision
   guards, when those schemas are integrated. They do not exist at base fdd2fa6.
   The frozen Python number pool spans all four holders; this branch covers
   written checks and bill payments. Refund schemas remain an integration request.

Stop before merge; cross-module fixture and number-pool integration belong to
the lead and owning workers.
