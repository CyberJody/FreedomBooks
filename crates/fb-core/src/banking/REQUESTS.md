# Banking integration requests (owned modules deliberately left untouched)

1. **Payables historical-copy fixture:**
   `payables::tests::rebase_tests::populated_payables_copy_migration_preserves_every_row_and_reopens`
   compares an exact pre-0025 schema snapshot to the current table-name set. The new,
   empty `banking_*` tables are absent at that historical prefix, so the comparison
   fails before migration. Adapt that fixture to exclude later empty tables at the
   prior-schema comparison, then compare every preexisting row after upgrade and
   require the new tables to be empty. Banking's own populated pre-0050 and pre-0051
   upgrade tests, and the shared combined-copy fixture, pass. This worker did not
   edit any payables source or tests, as required by the brief.

2. **Ledger reversal error boundary:** call
   `banking::refuse_cleared_reversal(tx, company, &original.id.to_db())` from
   `services::reverse_journal_source_tx` at its existing banking TODO. The banking
   `reverse_journal` wrapper already composes that check with the public ledger
   writer. SQL blocks cleared/reconciled reversals through *every* caller today;
   the shared hook is needed to give direct ledger/sales/payables callers Python's
   detailed ValidationError instead of a database IntegrityError.

3. **Sales refusal wording:** the existing sales
   `queries::refuse_live_deposit` hook correctly blocks live-deposit and backdated
   receipt reversals. Delegate its preflight to banking's exported
   `refuse_live_deposit` (with the receipt's company) to reproduce Python's deposit
   number and release-date wording. The banking harness exercises that real
   preflight and the Rust tests also call the existing sales reversal directly.

4. **Later provenance integration:** this branch resolves deposits, receipts,
   sales receipts, invoices, bills, bill payments and general journals. Add the
   checks/credits/prepayments/transfer provenance providers when their sibling
   modules integrate, preserving their Python labels and number-search rules.

5. **L2 configured-only reclassification/open-check mismatch:** the existing
   `save_account` correctly allows an unused designated bank to change kind,
   following the accepted Python rule. `VIOLATION_QUERIES` still labels every
   designated non-ASSET account invalid (`designated bank outside its company or
   kind`). Thus the service-created book fails `check()`/reopen even before
   banking adds anything. Banking follows Python's section-side eligibility,
   including interest for a reclassified debit-side account. The targeted Rust
   test leaves this existing diagnostic visible, and the differential
   `banking-reclassified-designation` case matches Python. The owning L2/core
   job needs to align its integrity query with its approved reclassification
   behavior. No shared core query was changed by this worker.
   Once fixed, change the explicit known-diagnostic assertion in
   `interest_eligibility_follows_the_section_side_of_a_reclassified_unused_designated_account`
   to require a clean `check()`/reopen, retaining its financial behavior assertions.

6. **Shared duplicate-key rejection:** integration should apply
   `fb_db::parse_unique_json` (also exported from `fb_db::json_evidence`) to all
   stored-evidence reads and payload validators. The SQL UDF
   `fb_json_unique_keys(text)` is registered on every book connection and can be
   used by every module's insert/update/seal/open guards. Banking uses both in
   migration 0055 and its existing evidence predicates. The parser compares decoded
   key names recursively, so escaped and same-value duplicates are rejected too.

Bank-feed matching/acceptance belongs to Python's separate `bankfeeds/` app and
was not translated here. No provider calls or staged SimpleFIN posting were added.
