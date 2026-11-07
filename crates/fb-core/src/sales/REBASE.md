# Sales adoption of core-int aaa964f in the combined core-l3 branch

Sales and payables share the hardened ledger, directory, tax, document,
authorizer and comparer implementations. The aaa964f merge preserves both
modules' registrations, callbacks, row checks, dump schemas and adapters.
All implementation and verification writes stay in this integration worktree.

## Capabilities and open lifecycle

- Sales read helpers accept `&(impl fb_db::Query + ?Sized)`. Public balances and
  activity commands take `&mut Book` and read through one `Book::read` callback.
- All sales mutations require a Writer; commands compose with `Writer::atomic`.
  Document primitives use the core's own nested atomics. No callback issues
  savepoint SQL, PRAGMA changes, schema work or function registration.
- Raw Connection/Transaction handles are confined to dev-dependency guard fixtures.
  Product sales and harness code cannot access them.
- `fb-db/src/sales` registers the exact-money callbacks during Book configuration,
  before the authorizer and migrations, and during scratch-schema derivation.
  Book open/check run the full sales row checks, not just a trigger-name manifest.
  `sales::initialize` remains a check-only compatibility alias.
- fb-db does not depend on fb-core. Its sales guard arithmetic is a pure snapshot
  of the inherited document arithmetic, checked against domain calculations for
  strict types, extremes and rounding and against all frozen Python scenarios.
  Future arithmetic changes must keep these two evaluators synchronized.

## Numbering and definitions

Core 0001–0009 retains aaa964f's exact bytes. Existing sales 0010–0014 and
payables 0020–0025 retain their exact bytes. New 0015 and 0026 add the L3 core
adoption guards, yielding 22 migrations: core 1–9, sales 10–15, payables 16–22.
Every table/CHECK, index, trigger and view enters the exact-prefix versioned
definition hash; `name NOT GLOB 'sqlite_*'` is unchanged.

Unmerged files from the earlier L2a/L3 lineage must be recreated; old hashes are
never blessed. The populated sales fixture now identifies its prefix by the
0010_sales migration name (user_version 10), reconstructs its exact shared seals,
tables, triggers and views, then upgrades to 22 without changing any preexisting
row/key. Outstanding remains 7.00 and reopening is clean. The combined and
payables copy tests use the named prefix before 0025 (version 20), then upgrade
to 22. Populated core v6/v7/v8 encrypted copies also preserve all rows through v9
before opening through the final chain. No populated upgrade test was removed.

## Protected table matrix

Every table below is STRICT, WITHOUT ROWID. All UUID `id` columns have an explicit
BEFORE UPDATE OF id guard. On the invoice root, unchanged id assignments may
accompany a valid revision advance; actual changes are refused with IS NOT.
Append-only rows still refuse all UPDATEs. Seal composite keys are append-only. Every declared
unique key has INSERT collision protection. Every sales table except the invoice
head refuses all UPDATEs; the invoice head additionally checks displaced victims
on both its keys before allowing its normal one-step revision advance.

| Table | Unique keys beyond id | Retention |
| --- | --- | --- |
| sales_invoice | identity_id | retained; revision advances |
| sales_invoicerevision | invoice_id/number | append-only |
| sales_invoiceline | revision_id/position | append-only |
| sales_receipt | identity_id; journal_id | append-only |
| sales_receiptallocation | receipt_id/invoice_id | append-only |
| sales_receiptreversal | receipt_id; journal_id | append-only |
| sales_salesreceipt | receipt_id | append-only |
| sales_salesreceiptline | sale_id/position | append-only |
| sales_seal | kind/doc_id primary key | append-only |
| sales_seal_due | kind/doc_id primary key | append-only |

Existing immediate parent/ownership/completeness rules remain. New invoice income
accounts and items must be eligible at insertion; draft/void history may retain
an archived prior selection, while posting requires eligible selections. Sales
references continue to protect contacts, items, accounts and ledger/source history.

`sales/rebase_tests.rs` exercises hidden aliases, permanent identities, each unique
key against a distinct victim, ordinary/REPLACE UPDATEs, UPSERT, DELETE, INSTEAD OF
view propagation, recursion ON/OFF and complete unchanged book snapshots. It checks
every sales schema object's definition at check/reopen, UDFs under Reader/Writer,
new-parent eligibility, guard arithmetic and the populated-prefix upgrade. Existing
sales tests retain every named guard and late audit rollback through all commands.
Failing-first command logs are kept in the external run evidence directory.

## Differential dump and comparer

The existing strict core comparer is retained, including full ordered audit details
and separate punctuation-only collation ties. The required sales object has exact
invoice/history/header/line, receipt/detail/allocation/reversal and balance shapes.
Missing/extra fields, wrong types and duplicate document/history/line/allocation/
tax-member keys fail even when the two malformed dumps are identical.

Rust and Python normalize only typed generated ID fields. Invoice role suffixes
are line positions and remain exact; RCP/SR role suffixes are account IDs and map
to scenario refs. Only those generated-role groups are reordered after ID mapping;
ordinary source/role observations retain the core collation comparison. Free text,
amounts, request keys, business numbers and all non-ID audit fields remain exact.
`strict_sales.rs` corrupts every populated sales structural path and full audit
leaves. All core plus nine sales scenarios are required gates, including the
500-command sales random run and the core 1,000-entry run.

## Merge boundary

Stop before merge. The lead must perform the independent integration review and
rebase/integrate the other L3 module and parallel L1 permanent-key work separately.
Banking, credits/prepayments and presentation remain later owning-module work.
