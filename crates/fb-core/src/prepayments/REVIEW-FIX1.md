# L3b prepayments review fixes

Base: `f7f21d2`. Local branch `l3b-prepayments`; stop before merge.
Migration 0072 is additive. Migrations 0070 and 0071 are unchanged.

## Findings and regression coverage

1. SQL CDP destinations require the configured undeposited account or a designated,
   non-control bank. The original deposit detail guard requires the configured
   customer-deposit liability. Applications/refunds continue to use the original
   frozen liability; refund banks use today's designation. Configuration predicates
   run at insertion, not reopen. Tests: copied `review_bank_deposit_sql_requires_designation`
   and `review_undeposited_sql_requires_configured_account`,
   `fix1_sql_destination_and_liability_designation_matrix`, and
   `fix1_configuration_changes_preserve_history_and_original_liability`.
2. `prepayment_snapshot_ok` has separate schemas for deposit, application, refund,
   unapplication, refund void, and deposit void. SQL checks every relational field,
   nested snapshot ownership, amount, and actor existence. The same predicates
   run at insert, seal, and check/open. Core loads and harness stored-JSON decoding
   return controlled errors. Tests include all copied snapshot/actor regressions
   and `fix1_full_snapshot_schema_matrix_at_insert_seal_check_and_open`, which
   removes/mistypes every required field, checks nested snapshots, and restores
   schema guards after corrupting evidence to prove open-time checking.
3. Direct parent retention covers every FK below, without depending on ledger,
   audit, or setup references. Existing unconditional key/collision guards remain.
   Tests: copied refund-only actor regression,
   `fix1_every_actor_reference_retained_without_indirect_ledger_or_audit_refs`,
   and `fix1_every_foreign_parent_has_immediate_retention_and_permanent_keys`.
   Delete/rebind, fresh-key updates, replacement, and recursion ON/OFF are covered.
4. The comparer checks canonical bounded money, real canonical dates, methods,
   states, balances, snapshot/account ownership, invoice revisions, reversal
   links, and exact CDP/detail plus source-journal/lifecycle key sets. The copied
   identical-corruption regressions and an expanded matrix cover all collections.
5. `fb_db::prepayments::parse_prepayment_date` matches Python's calendar/basic/ISO
   week grammar and year bounds without sign/separator coercion. There are 220
   frozen Python `date.fromisoformat` differential vectors, including all five
   reported cases and near-valid mutations. `prepayments-date-parity.json`
   exercises the five cases, accepted/rejected nearby forms, and both messages.
6. The only sales production edit is the two prepayment invoice-void messages,
   including the lookup needed to interpolate the deposit number and the existing
   release date. Conditions and accounting behavior are unchanged.
   `fix1_invoice_void_messages_match_python` checks exact text; the date scenario
   compares both refusals against the frozen Python reference.

## Parent inventory

| New child | Parent references and protection |
| --- | --- |
| customerdeposit | receipt, company, liability account: direct retention; permanent parent keys. `estimate_id` is constrained NULL pending integration. |
| depositapplication | company, document identity, deposit receipt, invoice, invoice revision, receivables account, liability account, journal, actor: direct retention; permanent parent keys. |
| depositrefund | company, document identity, deposit receipt, customer, bank, liability account, journal, actor: direct retention; permanent parent keys. |
| depositunapplication | application, journal, actor: direct retention; permanent parent keys. |
| depositrefundvoid | refund, journal, actor: direct retention; permanent parent keys. |
| seal/due | journal ownership checked by seal guard; `(kind, doc_id)` FK retained by unconditional seal DELETE/UPDATE guards. Both composite key components are immutable. |
| frozen JSON | customer/company/actor/source IDs agree with their relational owners; the correct reversal parent key is mandatory. Snapshot labels stay historical. |

All five new document tables already reject every UPDATE/DELETE, and collision
guards cover all unique keys independently of recursive DELETE triggers. All
referenced parent primary keys are protected by the ledger, directory, document,
sales, or prepayment permanent-key guards. Migration 0072 adds immediate direct
DELETE guards rather than relying on deferred foreign keys or incidental use.

## Integration and verification notes

- Consolidate `fb_db::prepayments::parse_prepayment_date` with the credits parser
  and `fb_db::prepayments::validate_prepayment_number` with the checks job's shared
  number validator. The latter is used by service normalization, SQL evidence,
  and the comparer; there is one character/length policy.
- `fix1_populated_0071_copy_upgrades_preserving_every_prepayment_row` copies a real
  populated prior schema, defers FKs only while copying fixtures, migrates, compares
  every row/key, reopens, and rejects a tampered prior schema.
- `fix1_both_lock_orders_for_every_available_prepayment_race` forces both writer
  orders for application/refund, application/receipt, application/deposit void,
  application/invoice void, competing applications, and exact retries. Test-only
  commit/busy hooks verify actual lock contention without assumptions about which thread runs first.
- Existing estimate, banking, credits/check-number and cash-report integration
  requests in `TRANSLATION.md` remain outside this fix. Refund-versus-check races
  still need the checks module at integration.
- Four unrelated upgrade fixtures remain allowed by the brief; the final report
  names them. No tests in those modules were weakened or changed.
- Evidence is under `runs/2026-10-30/l3b-prepayments-fix1-*.log` in the project
  root. The copied reviewer tests failed before production edits; the added
  message and date-vector tests also failed first.

Suggested skill lessons: test table-specific required fields and ownership;
inventory all parent identities; distinguish mutable configuration eligibility
from historical evidence; compare complete collections even for identical dumps;
validate ISO grammar before numeric parsing; prove both actual writer lock orders.

## Final worker gate results

- Workspace: **337 passed, 4 allowed upgrade-fixture failures, 1 ignored**.
- Prepayments: **47 core tests and 3 comparer tests passed**; all ten copied
  reviewer regressions are green. The date test contains 220 Python vectors.
- Clippy on fb-core/fb-db/fb-harness/fb-money with all targets and `-D warnings`: clean.
- Required harness DB pair: **41/41 MATCH, zero failures/warnings**, three accepted
  punctuation ties. Both databases were dropped and absence verified.
- Preview/screenshots: not applicable. `Cargo.lock` was not committed. Stop before merge.
- Own command output: `runs/2026-10-30/l3b-prepayments-fix1-verification.txt`,
  `l3b-prepayments-fix1-workspace-verified.log`, `l3b-prepayments-fix1-clippy-final.log`,
  and `l3b-prepayments-fix1-harness-final.log` in the project root.
