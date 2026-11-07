# Requests reserved for other module owners

The brief forbids changes to core, sales and payables implementation files. These
requests are concrete and intentionally left to their owners/the lead.

1. **Payables historical-copy fixture, required for the workspace test gate.**
   `crates/fb-core/src/payables/rebase_tests.rs`, in
   `populated_payables_copy_migration_preserves_every_row_and_reopens`, compares
   `snapshot(&c)` for an old migration prefix against a latest-schema snapshot.
   The latest fixture now includes ten empty `credits_*` tables that do not exist
   in that prefix. No data differ. Compare every table that exists in the prior
   schema, and assert any later-only table is empty; retain the full comparison
   after `Book::open` upgrades it. The shared harness's analogous historical-copy
   helper has this adaptation. The module's own populated v22→v23 upgrade passes.
   Do not skip this test permanently or weaken nonempty-row comparisons.

2. **Payables refund-number review hook.** In
   `crates/fb-core/src/payables/payments.rs::prepare`, inside the existing CHECK /
   1–20 ASCII-digit reference condition, call
   `crate::credits::refuse_number_conflict(tx, co, bank_id, reference)?`.
   This exported read-only hook requires the existing Writer capability. It
   preserves the Python messages and reads checks/refunds/payments/deposit-refunds
   under the same book write lock. Existing SQL triggers added by 0060 protect
   both refund→bill-payment and bill-payment→refund insertion. Currently a stale
   payables review followed by a new refund is safely refused at insertion with
   `bill_payment_check_number`; the payables review itself must gain the earlier
   Python ValidationError refusal. Repro: refund check 1001, then review a bill
   payment by CHECK with reference `1001` on that bank. Nonnumeric references and
   21+ digit references are free text and must not call the hook.

3. **Sales invoice-void wording parity.** Existing sales hooks correctly refuse
   live credit and backdated invoice voids, but use generic messages. Replace the
   credit branch with
   `crate::credits::refuse_live_credits(tx, invoice_id, void_date)?` (exported,
   read-only, Query/Writer compatible). It returns the frozen messages with the
   live memo number and latest release date. The behavioral/date rule is covered
   by module and raw-SQL tests. A differential diagnostic produced one wording
   warning for the existing generic sales release-date message. The committed
   receipt-settlement scenario exercises valid sales void after release, and
   credit-owned refusal scenarios remain exact.

4. **Checks / prepayments sibling integration.** Their future migrations must
   install symmetric SQL number guards on their own tables and extend credit
   refund insertion guards to those tables once they exist. Call the exported
   `refuse_number_conflict` hook from their review/record paths. Credit refund
   services already discover both table families when present. When adding
   deposit applications, extend the credit SQL settlement view to include them;
   the application service already reads `prepayments_depositapplication` /
   `prepayments_depositunapplication` when installed. These tables are absent at
   the assigned base, so unconditional SQL references cannot be installed here.

No UI, Electron, plugins, real-account tests or merge are part of this branch.
