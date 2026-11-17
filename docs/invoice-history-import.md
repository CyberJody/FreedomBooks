# Historical CyberBrain invoices

The company-scoped accounting API can preview and apply a bounded batch of CyberBrain invoices. The importer uses the same invoice and receipt Writer services as interactive work; it never calls Stripe or Gmail.

- Original invoice numbers remain visible in lists, customer activity, receipts, registers, email/payment descriptions and print views. Internal identity numbers are retained, and future invoice numbering cannot collide with an imported number.
- Charge, discount/credit and informational note lines preserve signed exact amounts, source tax flags, original quantities, descriptions and tax totals. A negative non-taxable invoice line does not independently reclassify earlier tax liabilities.
- Paid invoices create their invoice accounting and recorded receipts. Drafts stay unposted. A sent void is posted and reversed; an unsent void stays outside the ledger. Source timestamps and immutable evidence remain distinct from the new actor audit.
- Original bill-to text, line labels, subtotal and invoice discount appear while the financial revision still matches the source. Editing an imported draft switches presentation to its current content; the original evidence remains readable.
- Preview executes and rolls back the whole batch. Apply requires the connection-bound preview and unchanged account/customer/item mappings. Duplicate source identities, conflicting invoice numbers, payment references, incomplete evidence and mismatched amounts fail closed. Reapplying identical completed imports is a no-op.
- Migration 0193 adds invoice adjustments, 0194 permits canceling an unposted draft, and 0195 adds immutable import provenance and completion guards. Retained migration bytes are pinned and old-book backup, upgrade and reopen behavior is covered.

## Boundaries

Import supports the supplied invoice history, including source JSON payment references and internal notes. Existing provider links remain historical evidence, not active links. Imported card receipts use Undeposited Funds; settlement deposits and processor fees are separate records and must not be guessed from gross payments. The importer does not infer missing account mappings, start recurring billing, or recreate an external rolling-discount policy.

The production migration requires an unlocked company-specific Accounting write connection, a verified pre-upgrade backup and a fresh duplicate review. Private exports and company mapping files stay outside Git. Development and UI checks use isolated synthetic books only.
