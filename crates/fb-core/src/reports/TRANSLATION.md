# L4 reports: translation notes

Source: `ledger/financial.py`, `ledger/reports.py` and the data logic of `ledger/report_views.py`
(tag `python-reference-20261030`). Rust: `reports/mod.rs` (register, trial balance), `reports/financial.rs`
(P&L, balance sheet, general ledger, accrual and cash basis) and `reports/params.rs` (presets, fiscal year,
report-control validation, display rows, CSV cells, ledger paging).

No migrations: the reports are read-only and use the existing settlement tables. Each report runs in one
`Book::read` (the Python view's REPEATABLE READ, READ ONLY snapshot) and checks company membership first.

## Cash-basis slots filled

The prepayments and credits integrations (INT-L3b, prepayments TRANSLATION item 5) left the cash-basis
application/unapplication hooks to this layer. `financial::events` reads all five Python settlement families:

| Family | Settles | Date | Release |
| --- | --- | --- | --- |
| 0/1 receipt allocation (RCP) | invoice | receipt journal | receipt reversal journal |
| 2/3 bill-payment allocation | bill | payment journal | payment reversal journal |
| 4/5 credit application line | invoice AND credit memo | application date | unapplication date |
| 6/7 credit refund (RFD) | credit memo | refund journal | refund void journal |
| 8/9 deposit application (CDA) | invoice | application journal | deposit unapplication journal |

`posted_lines` marks the cash exclusion with the same rule as `cash_excluded()`: journals of INV/BILL/CM
documents, and the A/R or A/P control line of RCP/BPAY/RFD/CDA journals (originals and reversals, which keep
the source identity). `unconverted` uses `fb_control_accounts` with Python's highest-priority role.

## Ordering and identities

Python orders events by `(date, family, row pk)`, breaks cent-rule ties by account pk and sorts general-ledger
rows by `(date, journal pk or 10**15, position or 10**6 + family, pk)`. Rust uses the UUIDv7 row ids (creation
order) for row and account pks and the journal number (the J-number sequence that mirrors the journal pk) for
journals. The stable sort keeps the invoice event ahead of the memo event of one credit line, as Python does.

## Python oddities copied

- Credit applications/unapplications have no journal-status filter (they have no journal); every other family
  requires a POSTED journal, and a release counts only when its journal is POSTED and dated through `end`.
- A document's recognisable lines sum every original (non-reversal) posted journal of its identity; its control
  account is the control of its earliest event.
- An account with posted activity in a P&L period appears even when its net is zero; balance-sheet rows only
  when non-zero. Accounts sort by code code-point order then pk (`sorted`, not the database collation).
- The cent rule applies to negative cumulative amounts too (floor towards minus infinity, no negative leftover).
- A non-custom preset replaces typed dates; with no preset, any typed date means "custom".
- Only the exact empty string is a blank date: whitespace-only input is "Enter a valid date.".
- Retained earnings are computed with the report's own books (end = as of), so they never include later events.

## Changes required by the Rust design (not Python bugs)

- The general-ledger account control takes an account id (UUID) instead of Python's decimal pk; non-ids keep the
  message "Choose an account from the list.". Drill-downs are typed `Drill` targets instead of URLs.
- Preset arithmetic past 9999-12-31 returns a validation error where Python raises OverflowError (HTTP 500).
- HTTP, templates, print sheets, CSV response headers and provenance labels (`_describe`) stay with the
  presentation layer.

## Python tests translated

- `test_reports_units.py`: all cent-rule, preset, fiscal-year and display cases (`reports/tests.rs`).
- `test_reports.py`: the arithmetic and isolation assertions run as harness parity on every checkpoint of the
  combined company (both bases, fiscal-year boundaries, empty periods, single days, from-the-beginning ranges)
  plus `fb-harness/src/financial_reports_tests.rs` (balanced on every date, net income equals the fiscal-year
  P&L, retained earnings at the boundary, GL closing equals the balance sheet, company scope, unknown
  account/group). HTML, print, CSV-response, query-count and PostgreSQL snapshot tests are presentation tests.
