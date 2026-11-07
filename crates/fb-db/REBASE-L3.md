# Rebase checklist: L3 base 3484921 to core-int

This is the complete public API and behavioral adoption checklist relative to
`l2a-docs-base` **3484921**, the L3 jobs' base. Rebase onto the reviewed core-int
commit, then apply these changes in each L3 worktree. CORE-INT does not merge or
modify the L3 branches. Read `GUARD-PATTERNS.md` as well.

## Callback handles and capabilities

- [ ] `Book::write` now passes `&fb_db::Writer<'_>` rather than
  `&rusqlite::Transaction<'_>`. `Writer` has no public constructor or Connection
  dereference. It proves the owning `BEGIN IMMEDIATE` transaction exists.
- [ ] `Book::read` now passes `&fb_db::Reader<'_>` rather than `&Transaction`.
  Add `Reader` to explicitly typed read closures. Reader cannot become a Writer.
- [ ] `Book::{connection,connection_mut}` are available only with the `test-support`
  feature (or fb-db's own tests). Use `Book::read` in production. Enable
  `fb-db/test-support` only in dev-dependencies for raw guard attack tests.
- [ ] Reader and Writer expose the SQL-only methods `prepare`, `query_row`,
  `execute`, `execute_batch`, `is_autocommit`. `fb_db::Query` exposes `prepare` and
  `query_row`; Reader, Writer and independent raw fixture Connections implement it.
  There are no configuration setters, raw handles, hooks or Transaction derefs.
- [ ] New `Writer::atomic<T,E>(&self, f: impl FnOnce() -> Result<T,E>) -> Result<T,E>`
  (`E: From<rusqlite::Error>`) provides library-owned nested rollback for a composed
  primitive. Call `tx.atomic(|| { ... })`, retaining the same Writer. It rolls
  back on Err or panic, including a panic caught by the outer callback. Only the
  library's fixed savepoint-control statements run with library privileges.
  Document primitives use it already; callers do not issue savepoint SQL.
- [ ] Callback SQL now denies transaction/savepoint control, all PRAGMA writes,
  DDL/TEMP objects, ATTACH/DETACH, schema-table DML, REINDEX/ANALYZE,
  `load_extension` and `sqlcipher_export`. Reads also deny all DML. Ordinary
  authorized SQL and UDF/index/trigger execution still work. Move schema work to
  migrations and setup work to the library boundary; compose one Writer.

## Every changed core function signature

Each read function below changes its first parameter from `&Connection` to
`&(impl fb_db::Query + ?Sized)`; remaining arguments and return types are unchanged.
This accepts both Reader and Writer. Apply the same signature to L3 read helpers.

- [ ] `store::{user,is_member,company,get_company,account,accounts,account_has_lines,
  account_code_taken,journal,journal_in,lines}`.
- [ ] `services::{authenticated,access,next_entry_number_tx,journal_snapshot}`.
- [ ] `directory::{get,list,customer_tax_hints,prefill_catalog}`.
- [ ] `documents::{identity,source_accounts,control_role,purchase_line_allowed,
  line_account_error,eligible_line_accounts,setup_snapshot}`.
- [ ] `salestax::{settings_for,item,list,group_rate,item_label,tax_snapshot,
  same_snapshot,document_tax,bulk_preview,check_document_tax}`.
- [ ] `party_display::{duplicate_active_display_names,contact_label_map,
  account_mapping_rows}`.

These mutating functions change their first parameter to `&fb_db::Writer<'_>`;
other arguments and return types are unchanged:

- [ ] `directory::save_record_tx` (formerly `&Connection`).
- [ ] `documents::{create_identity,lock_configuration,snapshot_source_accounts,
  post_source,reverse_source}` (formerly `&Transaction`).
- [ ] `services::{audit,save_draft_tx,post_journal_tx,reverse_journal_tx}` (formerly
  `&Connection`). Crate-internal `save_draft_source_tx`, `post_journal_source_tx`
  and `reverse_journal_source_tx` also require Writer.
- [ ] `store::{insert_company,update_company,add_member,insert_account,update_account,
  next_journal_number,insert_journal,update_journal,insert_line,delete_lines}`
  are now **crate-private**, and require Writer. External crates use typed services.
  Existing typed Book-taking commands retain their signatures.

## Schema, guards, casing and observable behavior

- [ ] Schema version is **9** (`MIGRATIONS.len()`, recorded as `user_version`).
  L1 owns unchanged 0001 ledger, 0002 review hardening and 0003 callback/collision
  hardening. L2a is now 0004 directory/documents/tax, 0005 source journal hooks,
  0006 L2a hardening, 0007 L1 permanent identities and 0008 L2a permanent
  identities; 0009 applies L2a review fixes. L3 migrations start at **0010**. Old L2a migration indexes
  `[1]/[2]` at 3484921 become `[3]/[4]`; hardening is `[5]`.
- [ ] Public metadata constants `MIGRATIONS`, `REQUIRED_TRIGGERS` and
  `VIOLATION_QUERIES` now describe the combined hardened chain. Open/check also
  reject invalid retry evidence/revisions, over-limit manual totals, invalid
  directory/setup/bank account references, inactive active-group members and
  inactive default tax items. `REQUIRED_INDEXES` retains its existing interface.
- [ ] Recreate synthetic books from the old, unmerged L2a/L3 migration lineage.
  Do not attempt to bless their old hashes. Populated L1 books at versions 1–3
  upgrade through the numbered chain; the populated v2-to-v9 copy test preserves
  every existing column/key/row and requires new `source_id` values to be NULL.
- [ ] Protected L1 and all eleven L2a UUID tables are `STRICT, WITHOUT ROWID`.
  Remove every hidden `rowid`/`_rowid_`/`oid` or `last_insert_rowid` assumption.
- [ ] **Primary keys and ownership identities are permanent; add BEFORE UPDATE OF
  guards to every module table.** Cover every composite-key part, company_id,
  document/source ownership and link parents, even before references exist.
  Use `NEW.column IS NOT OLD.column` so no-op assignments remain possible.
  Do not freeze editable defaults (tax agency, default accounts/tax choices).
  Keep tax-member/bank set editing as DELETE + INSERT inside one Writer.
  Add fresh-key, valid-parent and rekey/reinsert regressions with recursion
  ON/OFF; uniqueness or deferred-FK errors must not mask a missing guard.
- [ ] Guard INSERT and UPDATE collisions on **every** unique key, including a
  displaced immutable/referenced victim of an editable source. Copy the direct
  and INSTEAD OF view matrix, ON/OFF recursion, UPSERT, hidden identities,
  delete/reinsert, parent-ordering and late-audit rollback cases.
- [ ] Directory records, setup, settings, counters, identities and snapshots are
  retained; archive or update via services. Tax-group member and designated-bank
  rows remain mutable sets, intentionally deleted/reinserted inside one Writer.
  Extend account/company/reference guards for the new L3 relationships.
- [ ] Protected-key retries must read and verify prior evidence. Do not depend on
  `ON CONFLICT DO NOTHING/UPDATE` bypassing a BEFORE INSERT guard. L2a's sequence,
  setup, tax and settings services now use explicit read/INSERT/UPDATE under Writer.
- [ ] Duplicate journal UPDATE collisions now report `fb_journal_guard`, including
  duplicate entry numbers; direct-SQL tests must not require the old `unique:*`
  error. The source migration retains all L1 collision keys plus the partial
  original-per-source key. Service validation messages retain differential parity.
- [ ] Schema hashes cover every non-internal table/CHECK, index, trigger and view
  from all eight numbered migrations, before and after upgrade. Preserve the exact
  `name NOT GLOB 'sqlite_*'` prefix filter, including near-reserved/case variants.
  Never repair tampered prior schemas or bless stored hashes.
- [ ] `fb_db::collation` is now public. New public functions are
  `compare(&str,&str)->Ordering`, `punctuation_tie(&str,&str)->bool`,
  `lower(&str)->String`, `upper(&str)->String`; `register(&Connection)` is newly
  externally reachable. It is for independent fixtures, never Book callbacks.
  Book open registers `fb_text`, `fb_lower`, `fb_upper` before the authorizer and
  migrations. Scratch schema derivation registers the same definitions.
- [ ] ICU4X compiled data replaces platform locale sorting/casing; use `fb_text`
  for L1 and later text orderings. Only tertiary shifted punctuation ties may
  differ from the Python/glibc oracle. Case/accent/letter differences still fail.
  Item and tax-name uniqueness use portable simple lowercase; service validation
  also checks uppercase, preserving Python's validation behavior for dotless i and
  Kelvin sign. See `COLLATION.md` and its documented exotic-alphabet limitations.
- [ ] Existing date helper signatures are unchanged, but `py::model_date` now
  accepts Python ISO week dates and Unicode-digit regex fallback; `py::form_date`
  accepts the translated Django en-US month-name/Unicode date forms. Journal
  revision increments reject overflow/reserved maximum values.

## Harness API and dump contract

- [ ] `Report` adds public `collation_ties: usize`; update explicit struct literals.
  CLI summaries print this count separately from warnings.
- [ ] `Runner::dump` already took `&mut self` at **3484921**; there is no signature
  change relative to that base. Its implementation now reads through Reader.
- [ ] Full audit payloads are compared, including actor/name, target, detail and
  order. Normalization maps only typed generated identifiers and known reversal
  display numbers. Extend Rust **and Python** typed normalization for new L3 IDs;
  never normalize free text/amounts or substitute audit counts.
- [ ] `compare` now validates required fields, exact object keys and types,
  report-map membership, tuple lengths and duplicate row keys before indexing or
  normalization. Identically malformed dumps fail. The required `l2a` object has
  `directory,tax,settings,setup,identities,sequences,snapshots,results,source_journals`.
  Extend the validator and corruption tests alongside every L3 dump extension.
- [ ] Order differences are failures unless all inverted pairs pass
  `collation::punctuation_tie`; accepted pairs increment `collation_ties` and emit
  no warnings. Contents, company/model grouping, counts and duplicates stay exact.

No other exported function signatures, model fields or ID/money types changed
relative to 3484921. Internal read helpers also use Query; internal mutators use
Writer. The integration evidence and command logs live in
`runs/2026-10-30/core-int/` outside this worktree.


## L2a-FIX4 adoption (after CORE-INT2)

- [ ] Reject malformed optional values rather than coercing them to omitted/null.
  Setup omitted banks clear the set; malformed/null banks error. Ordinary omitted
  roles clear, omitted customer_deposits preserves, explicit null clears. A tax
  item_id omitted/null creates; a malformed edit id errors before any mutation.
- [ ] Put each composable writer's changes AND audit in Writer::atomic. The
  directory save_record_tx now does so; document primitives already do. Test an
  outer callback that catches an injected audit error and successfully commits.
  Book-taking tax/directory/setup commands use their owning Book::write boundary.
- [ ] Match SQL eligibility to service detail types, active/postable and control
  account restrictions. 0009 fixes vendor INSERT and changed-reference UPDATE;
  retained stale defaults remain editable. Test every DetailType at both layers.
- [ ] Preserve Python's unused-account rule: a default-only/configuration-only
  account may change kind; journals and frozen source snapshots still freeze it.
  Permanent identity/company guards from 0007/0008 are unaffected.
- [ ] Use checked revision increments. i64::MAX is valid stored evidence but is
  exhausted for mutations; return ValidationError without changing anything.
  0009 makes nonnegative i64 revision bounds explicit on setup/tax tables.
- [ ] JSON helpers validate members/scales before use. Arithmetic accepts scales
  through 28 only. Preserve the property regression and the review reproductions.
- [ ] The harness independently sorts Rust chart/TB/directory/tax/snapshot rows
  using the ICU/UTF-8 contract and requires identity before accepting Python
  punctuation ties. Never use an oracle exception to excuse a wrong Rust order.
- [ ] Number new L3 migrations from 0010, retaining 0001–0008 byte-for-byte.
  Populated L2a copies at v6/v7/v8 upgrade to v9 and reopen unchanged; reconstruct
  prior trigger definitions as well as removing additive triggers in fixtures.


## CORE-L3b: completed adoption through aaa964f

The final combined chain has **22** migrations: core 0001–0009, sales
0010–0015, payables 0020–0026. user_version counts entries, not filenames.
All 20 preexisting migration SQL files are byte-for-byte unchanged; new 0015
and 0026 supply additive L3 changes. Old unmerged L3-lineage books must be
recreated, never assigned a new version/hash to bypass validation.

- [x] Preserve the complete union of the migration and required-trigger registries;
  preserve both callback families, integrity checks and strict dump validators.
- [x] L1/L2a permanent identities cover every referenced customer/vendor/item/tax
  and document identity. Existing L3 UUID and composite-key ownership guards,
  WITHOUT ROWID tables, replacement matrices and retention rules remain active.
  The two mutable document roots now allow unchanged identity assignments during
  a valid revision advance, while IS NOT still rejects rekeying. Append-only
  history remains immutable. New positive no-op tests failed before this change.
- [x] Reject malformed invoice edit IDs before choosing create/edit. The new
  regression first reproduced a bool ID creating a new invoice. Payables already
  validates optional bill/supersedes IDs; both modules validate optional text,
  flags, dates, line identities and amounts. Omitted/null optional IDs retain
  their intended meaning; malformed types are never coerced to missing IDs.
- [x] Changes and audit remain inside Writer::atomic. Sales has only Book-taking
  public mutations, through command; payables uses write and its composable
  save_bill_tx additionally owns an atomic including append/audit. Existing
  public late-audit and caught-composed-error tests remain enabled.
- [x] SQL and service eligibility agree for all 15 DetailTypes in both modules,
  including unconfigured RECEIVABLE/PAYABLE accounts and control restrictions.
  New draft customer/vendor/item SQL regressions first showed archived new
  selections accepted by SQL but refused by services. 0015/0026 close those
  gaps while preserving previously selected archived history. Posting and VOID
  exact-copy guards remain independent. core-l3-hardening checks retention,
  refused new selections, reactivation and posting through both implementations.
- [x] Default/configuration-only account reclassification follows Python. The
  combined hardening scenario reclassifies and restores unused configured A/R;
  journals and frozen document-account snapshots still freeze historical kinds.
- [x] All six L3 revision increments use documents::next_revision. Tests first
  reproduced overflow panics, then cover save/post/void at i64::MAX for each
  module and require ValidationError with no row/ledger/audit changes.
- [x] Validated calculation helpers and scale limits remain inherited unchanged.
  The independent Rust ICU/UTF-8 order check runs before any Python punctuation
  exception. L3 dump comparison remains exact and grants no new order exceptions.
- [x] Rebuild populated upgrade fixtures on the final named chain, keeping the
  sales 0010 prefix (v10 -> v22), payables/combined pre-0025 prefix (v20 -> v22),
  and core copied prefixes (v6/v7/v8 -> v9 -> v22). Restore original definitions,
  assert absent later tables are empty, preserve all rows/keys/reports, and reopen.

Detailed evidence: CORE-L3B-VERIFICATION.md and the external core-l3b-final logs.
