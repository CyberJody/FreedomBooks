# Sales backend (L3)

Scope: `sales/invoice_services.py`, invoice posting/void lifecycle, `receipt_services.py`,
`sales_receipt_services.py`, the in-scope models/constraints, and current invoice balances.
The frozen Python app remains the answer key. No Django views/forms/templates are ported.

## Command boundary

All public writes take `&mut Book` and own one `Book::write` IMMEDIATE
transaction. There are no public autocommit or raw-connection write primitives. Reviews
also use the writer boundary so their reads see the same configuration as Python's
company-locked review. Review objects have private workflow/digest fields: a caller cannot
construct or deserialize a reviewed digest. Their public display is not authority.

`Book::create`/`Book::open` register the sales-owned exact-money UDFs before the
callback authorizer and run the combined schema/data checks automatically. `initialize`
is a compatibility integrity-check alias; it does not register functions or expose a
connection. Sales SQL callbacks and row scans live in `fb-db/src/sales/`, without a
dependency from fb-db to fb-core. The pure guard arithmetic mirrors the inherited L2a
calculation rules; native cross-checks and the differential harness keep it aligned.
Money is INTEGER cents. Service balances sum integer rows in Rust.

All reads use `Book::read` and shared read helpers accept `&(impl Query + ?Sized)`.
Mutating helpers require `&Writer`; composed sales commands use `Writer::atomic`.
No production sales or harness code accesses a raw Connection. Raw guard fixtures
enable `fb-db/test-support` only through fb-core's dev-dependency.

## Schema and seals

Migrations 0010–0014 are sales-owned, applied after hardened core 0001–0006.
Their on-disk versions are 7–11. Existing core migrations are unchanged. Financial revisions,
lines, receipts, allocations and reversals are immutable. All ten sales tables are STRICT, WITHOUT ROWID. UUID primary keys are permanent
(BEFORE UPDATE OF id); composite seal keys are append-only. Collision guards cover
every UNIQUE key before INSERT and UPDATE, including displaced victims and
INSTEAD OF view UPDATE OR REPLACE with recursion ON/OFF.
Referenced contacts/items/accounts/source evidence cannot be deleted/reinserted or replaced.

The final schema uses `sales_seal` and `sales_seal_due`, a deferred-FK build/validate/seal
protocol. Each root revision, receipt, reversal and owned journal must be sealed to commit;
children cannot be appended after sealing. A root-head seal requires a real sealed revision,
so fabricating the head seal cannot discharge its obligation.

Why separate tables: SQLite resolves functions in triggers even when their WHEN condition
would be false. Putting exact-money sales callbacks on L1's `fb_seal` broke otherwise valid
L1 seal operations on connections that had not initialized sales. Migration 0013 moves the
sales protocol to its own tables and preserves the earlier checkpoint's already-immutable
seals. A populated version-7 checkpoint upgrades through version 11 with every prior row/key
unchanged, an outstanding balance of 7.00 and a clean reopen. Old unmerged pre-core-int
lineage books must be recreated; their old hashes are never blessed.

The L2a control-role view is extended in 0012: a historical UNDEPOSITED receipt destination
remains a control account after Accounting setup changes.

## Verification map

- Invoice create/edit/review/post/void, retries, stale revisions and changed configuration:
  native tests plus sales-invoice-payments, sales-edits-reviews and sales-random-500.
- Payment partial/final/reverse, no overpayment/on-account credit, original A/R accounts:
  native tests plus sales-invoice-payments and sales-original-ar-split.
- Cash sales, both destinations, per-account aggregation, void and kind separation:
  native tests plus sales-cash-sales and the mixed 500-command random scenario.
- Snapshots, archived retention/refresh, text limits, zero totals, invalid decimal/date/type
  inputs: native tests, executed Python date vectors, sales-maximum-text and sales-refusals.
- Group rounding and exact retry after rate edits: sales-tax-group-retry.
- Python's unshown-catalog-metadata review oddity: sales-catalog-review-oddity.
- Two-connection duplicate payment and competing overpayment races: native tests.
- Every named sales guard message: executed raw-SQL attack manifest test. Also UPDATE,
  DELETE/reinsert refusal, every own-table UNIQUE-key REPLACE with recursive triggers ON/OFF,
  source-directory reference attacks, orphan/forged seals, and full-row rollback after an
  injected late audit failure through each mutating lifecycle entry point.
- The harness compares all revision/header/line snapshots, receipt detail, allocations,
  reversals, current balances, the ledger/register/trial balance and full ordered audit payloads. Strict shape
  validation rejects missing/extra fields, wrong types and duplicate sales rows,
  including identical malformed dumps. Only typed generated ID paths are normalized.
  Comparer mutation tests prove sales differences are actually failures. Native tests check
  the recorded receipt/sale audit payload shapes separately.

## Integration requests / later ownership

1. The core startup request is resolved: Book open registers/checks sales automatically.
   `REBASE.md` records the hardened-core adoption and executable verification map.
2. Banking owns actual deposit membership and must install its raw-SQL reversal/live-deposit
   and release-date guards when its tables arrive. Sales service hooks already read the
   Python-shaped banking tables when present; no banking tables are created here.
3. Credits/prepayments own their settlement and release-date triggers. Sales services read
   their Python-shaped live applications when present. Later module integration must verify
   combined over-settlement, void dependency and release-date backstops, not just isolated tests.
4. Customer deposits are in `prepayments/`; no CDP receive/apply/refund implementation is
   included. Its future migration must extend the shared Receipt kind/completeness protocol.
5. D4 adds non-posting estimates, immutable state transitions and accepted-estimate conversion
   in `estimates.rs`, with migration 0131 (after main's 0130), shared guards in 0132,
   and `sales-estimates-d4` differential coverage. Main's migration prefix is frozen;
   `tools/verify_desktop_upgrade.py` proves real d14de5e books upgrade to the same
   schema as a fresh desktop book.
   `review_receipt_form` projects the frozen receipt/cash-sale review without changing the existing
   review/record services. Aging/as-of statement reports belong to the reports wave.

## Copied oddities and deliberate safety differences

- Saved line prices/descriptions are not repriced when catalog snapshots are refreshed.
- Unshown catalog description/rate changes do not invalidate a cash-sale review; recording
  captures the then-current metadata, exactly as Python does.
- Exact create/record retries survive later archive/void/reversal and tax-rate edits.
- Invoice balances are derived; draft/void outstanding is null, never a competing paid flag.
- Generic INV identities without an Invoice root remain allowed by the Python foundation
  scenario; an existing invoice's journal must go through its owning lifecycle.
- Missing parents cannot be deferred into existence later. Forged head seals, REPLACE and
  delete/reinsert cannot rewrite protected history. These close the known reference holes.
- UUIDv7 replaces serial IDs; signed Django strings become opaque in-process review objects;
  SQLite's lack of deferred triggers/transaction IDs requires obligations and seals.

Local-only, test data only. No preview server or screenshots: this is backend work.
