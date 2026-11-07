# Payables translation (L3)

Reference: frozen Python `payables/` at `python-reference-20261030`.
Base: integrated hardened `core-int` (`efd81c1`), rebased from `3484921`.
This branch does not merge or modify parallel modules. Core migrations 0001–0006
are unchanged; payables retains its assigned 0020–0024 range and appends 0025
(schema user_version 12). Old unmerged L2a/L3-lineage books must be recreated.

## Core surface

- `save_bill`, `review_bill`, `post_bill`, `void_bill`: append-only draft revisions,
  frozen vendor/item/account evidence, reviewed posting, exact retry semantics,
  reference re-entry, and compensating voids.
- `review_payment`, `record_payment`, `reverse_payment`: record-only payments,
  1–50 explicit allocations, original payable accounts, partial/final payment,
  stale-review refusal, exact retries and immutable reversals. No money is sent.
- `get_bill`, `current_bills`, `bill_tracker`, `bill_calendar`: current revision and
  live-allocation contract, scoped tracker filters, due-date calendar and totals.
  `current_bills` uses three batch reads, not a query per bill. Aggregation is in
  Rust; the caller supplies today's date in its display timezone. Tracker filter
  inputs are the validated equivalents of the Python form's cleaned data.
- `receipt_draft_input`, `save_receipt_draft`: core-side extracted/corrected field
  intake. Explicit local vendor, local defaults and UUID request key; always a
  draft, never a provider call or a post. Supplier tax arrives as a gross-cost line.
  Read suggestions remain editable when printed arithmetic differs; this direct
  Save path refuses unresolved mismatches. The host owns the stored reading,
  vendor matching, consent, attachments and the separate “Keep as entered” UI/audit
  path. Those assistant/form workflows are not ported by this payables module.

Public writes use `Book::write` and `Writer::atomic`; reads use `Book::read`
and shared helpers accept `&(impl Query + ?Sized)`. No production payables or
harness code accesses raw connections. The composed `save_bill_tx` primitive
rolls back independently if its caller catches an error. Core callback authorizers
and PRAGMA checks remain intact; raw access exists only in dev-dependency tests. `Review` is an opaque in-process capability with a private digest;
transport signing/retention is a host concern, not a public constructor.

## Database safeguards

History rows, parents and alternate unique keys have INSERT/UPDATE/DELETE guards,
including REPLACE protection with `recursive_triggers=OFF`. Deferred seals enforce
complete revisions, allocation sets and source journals at commit. Exact journal
multisets preserve PostgreSQL `EXCEPT ALL` semantics using counts and set checks.
Foreign parents must exist immediately; active/postable eligibility is rechecked
when posting. SQL sums compare persisted integer evidence only.

The exact quantity × rate assertion uses the Rust `fb_payables_line_valid` UDF,
registered by fb-db at create/reopen and scratch-schema construction, before
migrations and callback authorizers. It uses bounded exact i128 fixed-point HALF_UP
arithmetic (no float or SQL multiplication), including overflow-sized input checks.
A foreign connection without that function fails closed on bill-line insertion.
All payables trigger names are registered in the existing open-time manifest.
The test `every_guard_message_has_an_executed_attack` extracts the actual migration
RAISE messages and requires an executed rejection for every one.

## Parity evidence

Seven differential scenarios cover lifecycle/re-entry, edits/refusals,
partial/final/reversed payments, snapshots and archived records, original AP
splits, 50 allocations, receipt drafts, Unicode/ISO dates and 500 seeded commands.
Dumps compare all six payables tables, balances, tracker/calendar data, document
account snapshots, ledger rows, full ordered audit payloads and outcomes against
frozen Python. Typed audit targets, journal/bill IDs, allocation bill IDs and
`payables:<account-id>` roles are normalized on both sides. Free text, amounts,
revisions and business numbers remain exact. The payables dump has a required
strict shape validator: wrong types, missing fields, duplicate keys, partial item
snapshots and missing balance/tracker/calendar membership fail even when both
dumps are identically malformed.
The adapters do not implement substitute bill/payment accounting.

Native tests cover grouped service contracts and raw-SQL attacks, failure injection
after partial writes through every public writer, competing payments, simultaneous
creation, missing guards, and the deferred-parent/REPLACE escapes. HTTP/forms and
visual tests remain outside this Rust-core translation.

Final evidence lives in the project run directory:
`runs/2026-10-30/l3-payables-rebase/` contains failing-first regressions, final
workspace/clippy/harness logs, normalized dumps and verification-summary.json.
The earlier source captures remain in `runs/2026-10-30/l3-payables/`.

## Integration requests / limits

1. Independent integration review and lead verification/merge remain required.
   This worker stops before merge. The parallel L1 permanent-key change is not
   included in the pinned base; all six payables tables already have their own
   unconditional BEFORE UPDATE OF id guards.
2. Combine numeric check-number uniqueness with future checks and customer
   deposit-refund modules. `fb_payables_live_checks` currently covers payables only.
   Reversal clearing locks likewise belong to the future banking layer.
3. Host/CoreBridge wiring, durable review-token transport, persisted receipt
   evidence and the host's “Keep as entered” override/audit remain separate work.

Vendor credits are not in the frozen payables app, so no new credit subsystem was
invented. P2-3 reminder orchestration/settings live outside payables; its shared
open/overdue balance predicates are represented by the tracker contract.

## Preserved oddities and platform differences

- Exact creation retries return the original bill even after posting or voiding.
- Item income-account data is historical provenance, not the purchase posting target.
- Frozen snapshots do not silently refresh/reprice; archived selections can be
  explicitly retained for editing, but cannot be posted while ineligible.
- Payments clear each bill's original frozen payable account, not today's setup.
- Only ASCII digit-only check references of 1–20 characters reserve check numbers.
- Paid-recent means paid on or after today minus 30 days, with no upper date bound.
- Python ISO basic/week date spellings and HALF_UP half-cent results are preserved.
- UUIDs replace serial identities; transaction seals replace PostgreSQL writer XIDs.
  Monetary columns are integer cents; quantity is fixed-point with four places.
- The Python deferred missing-parent escape is deliberately not copied. Immediate
  parent and posting checks have raw-SQL regression coverage. SQLite REPLACE is
  separately guarded because PostgreSQL does not offer that implicit-delete path.


## Integrated-core adoption and regression matrix

- Migration 0025 atomically copies all six protected tables to STRICT, WITHOUT
  ROWID, restoring every index/trigger. The mutable bill UPDATE guard checks its
  PK, identity and partial live vendor-reference keys; other tables reject every
  UPDATE. Every primary key is permanent. Migration 0026 allows an unchanged
  bill-root id assignment during a valid revision advance, using IS NOT;
  append-only rows still refuse every UPDATE. DELETE remains forbidden on all
  six tables. Existing parent-reference guards survive.
- Draft-line insertion refuses missing/foreign/inactive/nonpostable/revenue/control
  purchase accounts. Explicitly retained historical choices remain editable;
  posting rechecks active eligible parents. NULL accounts remain valid incomplete
  draft choices, as in the frozen services.
- Exact-prefix schema hashes include every payables table, index, trigger and
  view. Tests alter each definition and require check/reopen rejection. Open-time
  row checks additionally validate line calculations, current revision ownership,
  contiguous revision/line evidence, seals, totals, payment allocation ownership,
  live balances and source/reversal linkage. These are integrity checks, not
  signed history against a key holder capable of consistent external rewrites.
- `rebase_tests.rs` exercises primary/all alternate/simultaneous replacement
  collisions, ordinary UPDATE, UPSERT, DELETE, view-propagated UPDATE OR REPLACE,
  every hidden identity alias and recursion ON/OFF. Every rejected statement is
  followed by an exact whole-book snapshot comparison, including ledger/audit.
- Populated payables books built on the exact named prefix before 0025
  (user_version 20 in the final chain) migrate to version 22 with every column/key/row unchanged,
  including posted/voided bills, payments/reversals, users, memberships, journals,
  audit and seals. Tampered prior schemas fail before migration. The inherited
  populated ledger-v2 copy test also upgrades through the combined chain.
- Failing-first evidence covers hidden rowids, permanent primary-key assignments,
  ineligible draft accounts, caught composed audit failures and restored-schema
  row corruption. Core callback cleanup/capability/authorizer tests run unchanged
  against the combined schema; inherited late-audit tests cover every public writer.
- Portable fb_text/fb_lower/fb_upper remain registered centrally. Only the core
  punctuation-tie predicate permits differential order inversions; ties are
  counted separately from warnings. The harness DB-prefix guard explicitly
  accepts `fbv2_l3p_` in the driver, Python environment and runner.

## L3-payables-FIX1 (base fdd2fa6)

Migration 0027 leaves every prior migration unchanged. It requires object-shaped
vendor, company, item and account snapshots, including object-valued account
roles. Integrity checks enforce the same shapes. Reads validate stored snapshots;
stored numeric/array/object conversions return errors instead of unwrapping.
Remaining production unwraps consume validated required IDs, normalized payment
input, or calendar arrays constructed in the same function, not stored JSON.

Bill lines now persist nullable `origin_position`: the position in the immediately
previous revision of the same bill. A unique index and a BEFORE INSERT collision
guard prevent two lines from consuming one origin, including OR REPLACE with
trigger recursion disabled. Retention of an archived item or inactive purchase
account requires that exact prior line to carry that resource. Different current
and prior positions are allowed, so reordering still works. Posting and voiding
copy each line with its immediately prior position. Existing historical rows
upgrade with NULL provenance because the old schema never recorded it; subsequent
edits can identify their prior positions normally. The populated upgrade fixtures
compare every original column and separately verify the new NULL column.

Tracker search uses `fb_db::collation::upper` on both operands, matching the frozen
PostgreSQL simple-uppercase contract. `payables-filtered-tracker.json` compares 27
filtered result sets (bill IDs and all totals), covering Unicode, literal wildcard
characters, vendor/date/state filters and their combinations. The strict comparer
validates those result shapes and identities. Provenance is a SQL guard detail
absent from Python and is excluded from the differential document dump; native
SQL/service and populated-upgrade tests validate it directly.

Dev Master follow-up, outside this job's edit scope: sales invoice editing retains
`account_snapshot` and calls `as_object_mut().unwrap()` in `sales/invoices.rs`.
Sales JSON loading and other readers also retain unchecked stored-value accesses.
No lowercase substring search exists in the Rust sales module at this base:
Python invoice/estimate/receipt/sales-receipt search views are not exposed by its
current Rust query API. Preserve PostgreSQL UPPER semantics when porting those
filters. Sales code is unchanged by this fix.
