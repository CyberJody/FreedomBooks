# Credits translation

Frozen source: `wt/reference/credits/`, including the live PostgreSQL guard bodies
captured from a migrated synthetic harness database. No views/forms/templates are
ported at this layer. There are no vendor-credit models or services in this app.

## Services

- `review_credit_memo` → `record_credit_memo` → `void_credit_memo`.
- `review_application` → `apply_credit` → `unapply_credit`.
- `review_refund` → `record_refund` → `void_refund`.
- `get_credit_memo`, `memo_balances`, `available_credit`, `live_applied`,
  `live_refunded`, and `invoice_credits` supply the corresponding read behavior.

All mutations own `Book::write` and `Writer::atomic`; review capabilities are
opaque and bound to the workflow and current plan. Amounts are integer cents;
calculation, summing, bounds and overflow checks run in Rust. Migration 0060 adds
eight immutable record tables and two immutable seal/obligation tables, all
STRICT and WITHOUT ROWID. Collision guards cover every unique key, and every
update/delete is refused. Open-time checks independently derive arithmetic,
posting/evidence multisets, scopes, chronology, settlement limits and ownership.

Posting/reversal uses `documents::{snapshot_source_accounts,post_source,
reverse_source}`. Applications use `sales::invoice`, its current revision and
frozen A/R snapshot. Existing sales balance/receipt/void hooks already read
`credits_creditapplicationline` and `credits_creditunapplication`; no sales or
payables implementation file was changed. SQLite's IMMEDIATE writer replaces
the Python company/customer/invoice row-lock order and serializes commands.

## Preserved Python details

- Credit memos, applications and refunds have no draft/edit lifecycle. Review
  does not reserve a document number. Applied/refunded/voided history remains.
- Applying credit never posts a second journal. Unapply is also non-posting.
- Exact retries return the original record after unapply/void. Changed payloads
  or review capabilities are refused, and no duplicate audit is appended.
- A picked tax rate/group is bound by request identity for retry purposes;
  changing the live rate later does not invalidate an exact retry.
- A memo review binds displayed item code, line values, accounts and party
  details. Unshown catalog description/default sale-rate changes remain allowed
  and are snapped live at recording. A failing-first regression covers this.
- Historical archived customers can apply, refund, unapply and void credit;
  new credit memos require active customers/items. Ledger reversals still require
  active postable accounts. Applications need original A/R existence, not current
  account activity or the company's current A/R selection.
- A designated ASSET bank is accepted regardless of detail type when it has no
  current or historical control-account role. SQL uses `fb_control_accounts`,
  exactly as the service does; a failing-first detail-type regression covers it.
- Refund numbers trim ASCII whitespace only and reject Python Unicode 15.1 C*,
  Zl and Zp categories. A frozen 712-range table keeps SQL and Rust validation
  identical without depending on a newer platform Unicode database.
- Refund check numbers share the bank's number space with numeric CHECK bill
  payment references. Reversal/void releases the number; other banks are separate.
- Journal summaries truncate by Unicode character count at 240; full text remains
  in the immutable document and audit payload.

## Additional safety

Malformed optional IDs/booleans/collections are refused rather than coerced.
Missing parents are refused immediately; deferred-FK insertion ordering cannot
change their meaning. Over-limit application totals fail as a plain validation
error before any write. SQL also requires active eligible parents, matching the
services rather than copying weaker Python raw-DML eligibility checks. Permanent
identities, replacement victims, hidden identities and schema hashing follow the
shared v2 contract. Tests exercise all 33 named guard constraints independently.

## Checks

The module tests cover posting/reversal multisets, retry/review binding,
non-posting settlement, bank-number integration, malformed values, all mutation
audit rollback paths, replacement/UPSERT/view attacks, trigger recursion ON/OFF,
hidden IDs, permanent composite seal keys, populated v22 upgrades, restored-schema
corruption and competing connections. The differential adapter preserves full
records, balances and typed audit payloads. Strict comparer tests exercise 425
populated structural paths, missing fields, duplicates and audit leaf mutations.

The harness includes eleven `credits-*` scenarios: lifecycle, refusals,
review/original-A/R changes, picked group retry, maximum text, catalog review
metadata, multi-invoice application, exact check-number text validation,
retries with alternate scenario aliases,
receipts after credit settlement, and an
exactly 500-command seeded random run (plus setup).

See `REQUESTS.md` for changes reserved for the lead/other module owners. In
particular the unmodified payables historical-copy fixture requires an empty-table
inventory adaptation before the unrestricted workspace test gate can pass.
