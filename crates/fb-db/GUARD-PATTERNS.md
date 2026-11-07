# Database guard reference

Consult the relevant patterns when changing stored-data safety. Existing tests are the executable coverage; this is not a checklist to copy into every task.

Source-row UPDATE guards, DELETE
triggers and endpoint-only PRAGMA checks are not sufficient immutability.
Implemented ledger examples: `sql/0003_callback_and_collision_hardening.sql` and
`sql/0007_permanent_identities.sql`. Migrations 0001-0006 remain unchanged;
0003 copies old data forward, 0007 adds ledger permanent-identity guards and
0008 applies the same rule to L2a.

## 1. Reject replacement collisions before INSERT and UPDATE

SQLite REPLACE deletes conflicting rows. Its implicit DELETE triggers depend on
`recursive_triggers`; an outer `UPDATE OR REPLACE` also propagates its conflict
policy into an INSTEAD OF view trigger. Never depend on DELETE triggers to save
replacement victims.

Every protected table needs a BEFORE INSERT guard covering EVERY unique key, not
just the primary key. UUID-keyed tables must be `STRICT, WITHOUT ROWID`: otherwise
an occupied hidden `rowid`, `_rowid_` or `oid` plus a fresh UUID can replace an old
row without colliding on any declared key. Do not use hidden rowids in services.

```sql
CREATE TRIGGER thing_guard_insert BEFORE INSERT ON thing
BEGIN
  SELECT RAISE(ABORT, 'thing_guard: Protected key already exists')
  WHERE EXISTS (SELECT 1 FROM thing
                WHERE id = NEW.id OR (company_id = NEW.company_id AND number = NEW.number));
  -- Existing-parent, company, state, amount and eligibility checks follow here.
END;
```

The BEFORE UPDATE guard must also inspect OTHER rows that NEW's keys would
displace, even when OLD is unused or otherwise editable. Rejecting all distinct
collision victims is the conservative ledger policy; it does not depend on the
victim's DELETE trigger or on trigger recursion.

```sql
CREATE TRIGGER thing_guard_update BEFORE UPDATE ON thing
BEGIN
  SELECT RAISE(ABORT, 'thing_guard: Update would replace an existing row')
  WHERE EXISTS (SELECT 1 FROM thing
                WHERE id IS NOT OLD.id
                  AND (id = NEW.id OR (company_id = NEW.company_id AND number = NEW.number)));
  -- Then check OLD's immutability/ownership and NEW's other invariants.
END;
```

For composite primary keys, exclude OLD using the WHOLE original key. Include
partial-index predicates, collations and NULL semantics exactly as the unique
indexes do. A fully append-only table may simply refuse every UPDATE. Include all
of a table's unique keys in the same guard; do not depend on trigger firing order.

Do not rely on UPSERT or INSERT OR IGNORE bypassing a BEFORE guard. Resolve
legitimate protected-row retries by reading and checking immutable evidence, not
by replacing the row. Membership add-if-absent remains INSERT OR IGNORE: its
composite key has no hidden rowid or non-key payload to overwrite.

## 2. Primary keys never change; refuse DELETE of immutable or referenced rows

**Primary keys never change**, including unused rows, drafts and EVERY component
of a composite key. A collision check only protects the destination; it does not
prevent moving OLD to a fresh key and inserting a different row under OLD's key.
Deferred foreign keys accept that sequence while historical references silently
resolve to the replacement. Keep collision/DELETE guards and add this independent
BEFORE UPDATE OF guard for each protected table:

```sql
CREATE TRIGGER thing_primary_key_permanent BEFORE UPDATE OF id ON thing
WHEN NEW.id IS NOT OLD.id
BEGIN SELECT RAISE(ABORT, 'primary_key_immutable: Primary keys never change'); END;
```

For composite keys, list every component in UPDATE OF and OR together their
`NEW.key IS NOT OLD.key` comparisons. No-op key assignments may pass unless the
row is already wholly immutable. Use literal, plain RAISE messages. No guard may
depend on which trigger runs first. The dedicated key guard and the existing
guards can independently refuse the same change.

Review foreign-key and other identity columns too. L1's complete inventory is:

| Identity | Protection / intended changes |
| --- | --- |
| All six UUID primary keys | Unconditionally permanent, including users and unused accounts/draft lines. |
| Membership company_id/user_id; seal and seal-due kind/doc_id | Every composite key component is permanent. Membership revocation remains DELETE; a new membership is a new row. |
| Account company_id | Permanent even before the account has journal lines. |
| Journal company_id/created_by_id/created_at/number | Existing ownership guard makes these permanent from creation. |
| Journal reversal_of_id | Permanent from creation, including NULL on an original draft. |
| Journal request_key/creation_digest | Existing retry-evidence guard makes these permanent from creation. |
| Journal posted_by_id/posted_at | Assigned only when posting; draft values must be NULL and posted rows are wholly immutable. |
| Line journal_id | Permanent even for drafts; edit/recreate content within its original journal. |
| Line account_id/position | Draft content may change to an eligible account/position; posted lines are wholly immutable. Referenced accounts cannot be rekeyed or deleted. |
| Audit company_id/actor_id/target and payload | All audit columns are append-only. Referenced operators/companies cannot be rekeyed or deleted. |

Names, account codes and draft entry numbers are editable labels, not referenced
primary identities. Their unique-key collision guards still apply. Extend this
inventory whenever adding a referenced identifier or frozen snapshot.

Keep unconditional UPDATE/DELETE guards on append-only rows. Referenced parents
need an immediate DELETE guard: deferred foreign keys alone allow DELETE then
reinsert of the same key, changing historical references before COMMIT.

```sql
CREATE TRIGGER thing_guard_delete BEFORE DELETE ON thing
WHEN OLD.status = 'POSTED' OR EXISTS (SELECT 1 FROM child WHERE thing_id = OLD.id)
BEGIN SELECT RAISE(ABORT, 'thing_guard: Referenced or posted rows cannot be deleted'); END;
```

Extend referenced-row tests when adding references or frozen account snapshots.
At line insertion/update, require an EXISTING draft parent and EXISTING eligible
account with NOT EXISTS (eligible parent join), not EXISTS (invalid parent join).
At posting, LEFT JOIN accounts and treat a NULL account as invalid. Deferred FKs
and INNER JOINs must not hide missing parents. This also closes an inherited
Python raw-SQL defect; the frozen reference remains untouched.

## 3. Authorize the whole callback, not just its endpoints

Every public command uses Book::write. It verifies required PRAGMAs BEFORE taking
its IMMEDIATE writer capability and again BEFORE COMMIT: foreign_keys=1,
recursive_triggers=1, synchronous=FULL, journal_mode=WAL,
ignore_check_constraints=0 and writable_schema=0. Bad state is refused, not
silently repaired and used.

These checks are only defense in depth. fb-db installs rusqlite's SQLite authorizer
(`hooks` feature) and activates a restricted policy for the ENTIRE callback:

- Write callbacks allow ordinary reads and DML, but deny schema-table DML.
- Read callbacks deny all INSERT/UPDATE/DELETE as well as the restrictions below;
  query_only=ON is an additional engine-level defense, not the boundary by itself.
- Both deny BEGIN/COMMIT/END/ROLLBACK and all SAVEPOINT/RELEASE/ROLLBACK TO commands.
- Both deny ALL PRAGMA writes, including recursive_triggers OFF/ON,
  defer_foreign_keys, writable_schema and query_only. There is no callback write
  allow-list. Only documented, argument-free diagnostic PRAGMA reads are allowed.
- Both deny all CREATE/DROP/ALTER (including TEMP/virtual objects), ATTACH/DETACH,
  ANALYZE/REINDEX and unknown authorizer actions. load_extension and
  sqlcipher_export functions are denied as well.

The library alone runs setup PRAGMA writes (key, journal_mode, foreign_keys,
recursive_triggers, synchronous), migration user_version changes and read-mode
query_only changes; callbacks have no access to that internal policy state.
Library transaction completion/rollback happens after the callback policy scope
ends. Error and panic unwinding reset the policy before rollback; read unwinding
also restores query_only. Migrations run only through the private migration path,
never inside a document callback. Schema/data-copy migrations are atomic.

Keep the raw connection crate-private. The test-support escape hatch is ONLY for
dev-dependency attack tests, never a normal/product dependency. Do not reintroduce
Connection or Transaction derefs: safe non-SQL methods such as set_db_config can
switch triggers off without invoking an SQL authorizer. Reader and Writer expose
only prepare, query_row, execute, execute_batch and is_autocommit; SQL still passes
through the authorizer. There is no raw handle, configuration setter, hook setter,
public constructor or mutable-connection access through these handles.

CORE-INT additionally exposes `Writer::atomic` for nested document primitives.
Only fixed, private library savepoint statements temporarily use library policy;
the nested closure retains callback policy throughout. Err and panic roll back
the primitive. Raw SAVEPOINT/RELEASE/ROLLBACK TO SQL remains denied, including
inside `atomic`. No user SQL or closure executes with library privileges.

Registered native functions and collations must be trusted and side-effect-free.
They must never change connection configuration/authorizers/hooks, disable guards,
or expose/capture a writable connection. SQL authorization does not mediate native
configuration calls inside these callbacks. Before adding a UDF or collation,
add boundary tests proving these requirements in both read and write callbacks;
do not treat an allowed SQL function name as proof of native callback safety.

## 4. Versioned schema-definition hashes, not name checks

Book::open compares SHA-256 of every non-internal sqlite_master SQL definition
(tables/CHECKs, indexes, triggers, views) against a scratch schema constructed from
numbered MIGRATIONS. It checks the prior version BEFORE migration and the current
version afterward, so migration cannot silently repair evidence of tampering.
Missing, changed and extra objects fail closed.

The filter is the literal prefix `name NOT GLOB 'sqlite_*'`; underscore is literal
in GLOB. Never use `LIKE 'sqlite_%'`: that underscore is a wildcard and hides legal
user objects named sqliteX..., sqlite-..., SQLiteX..., etc. Test case variants.

Add numbered migrations; never bless the current file's hashes. Add data-violation
queries for invariants that require rows (totals, retry evidence, reversals, etc.).
This detects violations, not every consistent rewrite by an external schema owner
holding the encryption key. It is not signed history or a boundary against such a
key holder. Callback schema restoration attacks are now denied, not merely checked
on reopening.

## 5. Transaction capability and a SQL-only read surface

Only Book::write constructs fb_db::Writer. It owns BEGIN IMMEDIATE and commits on
Ok, rolls back on Err (including a deferred COMMIT failure). Multi-row _tx services
and mutating store helpers require &Writer, never &Connection, &Reader or a generic
rusqlite Transaction. Unchecked row writers remain crate-private. Propagate failed
sub-operations with ? rather than catching and committing partial work.

Compose document commands inside one Writer; do not open nested commands or
transactions. Reader/Writer have no Connection deref. Book::read's callback receives
&fb_db::Reader, not &rusqlite::Transaction. Shared read helpers take
`&(impl fb_db::Query + ?Sized)`; Query exposes prepare/query_row, not connection
configuration. It is implemented by Reader, Writer and standalone Connection (for
independent fixtures). Query is not a write capability, even though prepared SQL
is supported: the Book callback authorizer enforces read/write mode. Tests must
prove configuration/hook APIs cannot be reached from either callback handle.

## Regression coverage reference

Use real synthetic SQLCipher books. The executable L1 examples are
`fb-core/tests/collision_matrix.rs`, `followup_review.rs`, `followup_extra.rs`,
`identity_permanence.rs` and `fb-db/src/fix2_tests.rs`. Reuse or extend the relevant coverage for new tables, keys and commands. Verify persisted behavior; avoid duplicating tests that already exercise the same invariant.

| Axis | Required cases and assertions |
| --- | --- |
| Source vs victim | Editable/unused source onto an immutable/referenced victim; isolate PK and EVERY alternate/composite/partial unique key, then simultaneous collisions. |
| SQL entry point | Ordinary UPDATE, INSERT OR REPLACE/REPLACE, UPDATE OR REPLACE, UPSERT, DELETE/reinsert; propagate UPDATE OR REPLACE through an INSTEAD OF view trigger. |
| Engine setting | Run raw guard attacks with recursive_triggers ON and OFF. Assert statement refusal and exact unchanged row snapshots, account/register meaning and audit history. |
| Permanent source identity | Change each PK component to a FRESH unused key, then reinsert a replacement under the original key. Include referenced operators and unused/draft source rows, ordinary UPDATE and UPDATE OR REPLACE, and recursion ON/OFF. Assert immediate refusal, exact unchanged snapshots, original actor/membership/report meaning, check() and reopen. Isolate one source row so a sibling key collision cannot accidentally satisfy the test. |
| Foreign identities | Inventory every FK/ownership/retry identity and verify permanent ones cannot move. Test posted account/operator references and both draft and posted line ownership; prove legitimate draft-content edits and posting still work. |
| Hidden identity | Assert WITHOUT ROWID; attack occupied rowid, _rowid_ and oid using a fresh UUID, for both INSERT and UPDATE replacement. Cover every protected table. |
| Parent ordering | Missing parents inserted later, cross-company parents, inactive/non-postable accounts; refuse or roll back the entire operation. |
| Callback scope | Attempt transient PRAGMA OFF/ON, defer_foreign_keys ON/OFF, writable_schema, query_only OFF; attempt every transaction/savepoint command, DDL and ATTACH/DETACH from BOTH read and write callbacks. |
| Native callback trust | Before registering functions/collations, prove they cannot mutate configuration, replace hooks/authorizers or retain writable-connection access in either callback mode. |
| Callback cleanup | Insert a marker then attempt early COMMIT and return Err; no durable marker after reopen. Test ordinary Err, deferred-COMMIT failure and panic unwinding; prove later reads/writes still work. |
| Capability | Compile-fail tests for raw Connection/configuration/hook access and passing raw/autocommit/read handles to multi-row writer helpers. |
| Schema | Drop/no-op same-name trigger/index/CHECK, unexpected view/trigger and sqliteX/sqlite-/case-variant near-reserved names; check and reopen must refuse. |
| Migration | Migrate populated prior-version books with posted journals, lines, reversals, users, members, audit events and seals; compare all rows/keys and reopen; tampered prior schemas must not be repaired silently. |
| Atomic command | Inject a late audit failure through EVERY public write entry point and composed _tx path; verify all associated document/ledger/audit rows rolled back. |
| Comparer | Reject malformed types, missing required keys, duplicate keys/rows, missing/extra reports and corrupted values; include IDENTICAL malformed dumps on both sides, not only unequal sides. |

When extending a differential dump, extend its strict structure validator and
comparer self-tests in the same change. Null/scalar/array report maps must fail,
not be skipped, warned about, defaulted or allowed to panic. Validate required
fields and tuple element types before indexing or normalizing any dump.

## Historical adoption notes

The following records past integrations. It is not an instruction to repeat rebases, renumber retained migrations or reopen completed work.

### Changes since 4a59131

Downstream L2a and L3 must make exactly these adoptions (none of their worktrees
was changed by L1-FIX2):

1. Rebase on this L1 fix. Retain migration 0003_callback_and_collision_hardening
   after 0002; renumber downstream migrations if they already occupy 0003. Do NOT
   edit L1's old migrations or append downstream tables before L1's rebuild.
2. Use STRICT, WITHOUT ROWID on protected UUID-keyed tables. Remove rowid aliases
   and last_insert_rowid assumptions; use explicit UUID keys. Migrate already
   created rowid tables by atomic copy, preserving all references/data/guards.
3. Add collision-victim checks to every new BEFORE UPDATE guard and retain
   all-key BEFORE INSERT checks. Cover every unique index including partial keys
   and its NULL/collation semantics. Ordinary immutability of OLD is insufficient.
4. Keep &Writer for mutating helpers. Change read-helper parameters formerly
   &Connection (or &Transaction) to &(impl fb_db::Query + ?Sized), and import Query
   where generic methods are used. No change is needed at ordinary inferred
   Book::write/read call sites using the supported SQL methods.
5. Explicitly typed Book::read callbacks now receive &fb_db::Reader, not
   &rusqlite::Transaction. Writer no longer implements Deref<Target=Connection>.
   Never add back raw connection access to make downstream signatures compile.
6. Move callback PRAGMA changes, transaction/savepoint control and DDL into their
   proper library-owned boundary or a numbered migration. Callbacks cannot create
   TEMP objects, attach databases or change settings. Compose via one Writer.
7. Preserve the literal schema-prefix filter and versioned definition checks when
   adding migrations. Extend the strict dump schema and corruption self-tests for
   any new dump fields. Malformed dumps now return a failed Report, not MATCH or a
   panic. Duplicate-update collisions now report the named fb_* guard instead of
   SQLite's unique:* error; adjust direct-SQL error assertions accordingly.
8. Copy the required matrix above and prove populated upgrades and rollback using
   each module's real document states. Keep test-support in dev-dependencies only.

Public API changes: added Reader and Query; changed Book::read callback handle;
removed Writer's Connection deref; generalized store::{user,is_member,company,
get_company,account,accounts,account_has_lines,account_code_taken,journal,
journal_in,lines} and services::{authenticated,access,next_entry_number_tx,
journal_snapshot} read parameters from &Connection to &(impl Query + ?Sized).
Book::write and all typed mutating helper signatures are unchanged. The SQL-only
methods previously reached by Writer deref remain available as inherent methods.

## CORE-INT: L2a adoption and executable table matrix

Schema version **8** keeps L1 0001–0003 unchanged and applies L2a 0004–0006, then L1-FIX3 0007 and L2a permanent identities 0008.
The source-hook migration preserves the L1 all-key UPDATE collision guard and
adds the partial original-per-source key. All definitions enter the exact-prefix
versioned hash. The full API checklist relative to L3 base 3484921 is in
`REBASE-L3.md`; do not use the older L2a migration numbers or callback signatures.
Synthetic books from the old unmerged L2a lineage are recreated; populated L1
books upgrade normally. The populated v2 copy test now verifies through v8.

`fb-core/tests/l2a_replacement.rs` runs these eleven tables through the shared
matrix. Every row has a UUID/text primary key and WITHOUT ROWID; the second
column lists every additional unique key:

| Table | Alternate/composite unique keys | Retention |
| --- | --- | --- |
| directory_customer | none | archive, no DELETE |
| directory_vendor | none | archive, no DELETE |
| directory_item | fb_lower(code), company_id | archive, no DELETE |
| salestax_taxitem | fb_lower(name), company_id | deactivate, no DELETE |
| salestax_taxgroupmember | group_id/member_id; group_id/position | mutable link set |
| salestax_salestaxsettings | company_id | retained |
| documents_documentsequence | company_id/kind | forward-only counter |
| documents_documentidentity | company_id/number; company_id/kind/ordinal; company_id/request_key | immutable |
| documents_documentaccount | source_id/role | immutable snapshot |
| documents_accountingsetup | company_id | retained |
| documents_designatedbank | setup_id/account_id | mutable link set |

The tests cover INSERT/UPDATE replacement, ordinary UPDATE, UPSERT, INSTEAD OF
view UPDATE propagation, all hidden identity aliases, recursion ON/OFF,
delete/reinsert, missing/ineligible/cross-company parents and complete unchanged
book snapshots. Mutable links intentionally permit service delete/reinsert;
they have no child references, and their keys/parents still receive guards.
Late audit failure tests cover all 17 L2a mutation paths. L1's account/journal
matrix runs against the combined schema too.

`fb-db/src/core_int_tests.rs` proves portable collation and casing UDFs are ready
at create/reopen under Reader and Writer authorizers, including expression
indexes and collision triggers; trusted nested primitives retain authorization
and clean up after Err/panic; every L2a table, explicit index and view is hashed
at check/reopen, as are near-reserved prefixes on a directory trigger. Existing
L2a tests replace every added hardening trigger by a same-name no-op.
`fb-harness/tests/strict_l2a.rs` corrupts every populated L2a structural path and
tests identical malformed pairs before comparer normalization. Extend these
tests alongside each later module.

## Adoption since 15a0919 (L1-FIX3)

L1-FIX3 is integrated as 0007_permanent_identities after L2a 0004–0006.
The expected definition hashes are derived per version from MIGRATIONS: registering
0007 adds its twelve trigger definitions to the v7 hashes while retaining the
exact v1-v6 hashes.
No old migration or static trusted-hash list is rewritten.

Copy the permanent-PK guards onto every new protected table, including composite
keys, and review all foreign identities using the inventory above. The populated
v2 copy migration and the direct populated v3-to-v8 migration both compare every
row/key and reopen. Rebase/cherry-pick only through the lead; this worker does not
modify downstream worktrees.


## CORE-INT2: L2a permanent identity adoption

Migration 0008 adds eleven independent `BEFORE UPDATE OF id` guards and twelve
ownership guards. All eight company_id columns are permanent. Tax item kind,
sequence kind, tax-member group/member, designated-bank setup/account, frozen
snapshot source/account/role and journal source_id are permanent too. Defaults
(agency, tax selections, account mappings) remain editable through services.
Mutable tax-member and bank sets still permit service DELETE/INSERT; existing
retention rules are unchanged. New guards use `IS NOT`, permitting no-op identity
assignments wherever older full-snapshot guards allow updates.

The v8 definition hashes are derived from the complete numbered chain; v1-v7
definitions remain unchanged. All 35 guards introduced by 0007/0008 are required,
and tests drop/replace each definition and require check/reopen refusal.
`l2a_identity_permanence.rs` inventories every L2a primary key and company column,
tries valid alternate parents with ordinary/OR REPLACE updates and recursion
ON/OFF, and compares the complete book after each refusal. Failing-first tests
showed tax-member group/member and bank account reassignment, including a
reparent/reinsert sequence, were accepted before 0008.


## L2a-FIX4: input boundaries, nested audit rollback and exact eligibility

Reject malformed input; do not coerce it to missing/null/empty. Explicitly test
omission, null, bool, integer, string, array and object at optional boundaries.
Preserve Python's omission defaults, including customer_deposits preservation.
Malformed tax edit IDs must never select the creation path or quick-add an agency.

Each composable mutation includes its audit inside Writer::atomic. An outer
Book::write can catch an error and commit: failure must still roll back the
primitive's rows, sequence changes, journals and audit. Test all composable L2a
helpers this way; propagating errors only tests the outer transaction boundary.

SQL eligibility must match service rules exactly, including detail types,
active/postable flags and configured control roles. Broad base-kind checks do
not exclude unconfigured receivables/payables. Compare all detail types for new
INSERT and changed-reference UPDATE while preserving retained stale defaults.
Migration 0009 replaces only the affected guards; earlier hashes stay intact.

Keep identity permanence separate from historical kind permanence: defaults and
setup references are not historical usage in Python. Journal lines and frozen
document snapshots still freeze kind; unused default-only accounts may change.

Use checked revisions and controlled exhaustion errors at i64::MAX. Validate
public JSON and decimal scales before unwrap/construction, and property-test
untrusted helper inputs. Check deterministic Rust order independently before
applying Python punctuation exceptions. Upgrade tests must restore replaced
prior definitions, not merely remove new triggers.


## CORE-L3b integration lessons

When inserting core migrations ahead of a module, rebuild historical fixtures
from an exact named prefix. user_version is the applied migration count; never
carry a hard-coded index across registry insertions. Compare populated encrypted
copies before and after each intermediate core prefix and the combined upgrade.
Keep prior migration bytes unchanged and verify the union of metadata registries.

New directory eligibility belongs at the SQL boundary as well as the service;
preserve eligible historical retention separately from a newly selected archived
parent. Compare every DetailType through actual service and INSERT paths. No-op
identity assignments may accompany legitimate mutable-root updates; actual key
changes are refused with IS NOT. All six L3 revision mutations use checked
increments before changing snapshots, ledger rows, seals or audits.


## Module review lessons

- Validate stored JSON shapes and snapshot identities at insert, seal and open;
  return controlled errors instead of unwrapping stored evidence.
- Retention eligibility follows validated prior-line origin provenance, never
  the new position. Reject duplicate origins even when the lines are reordered.
- SQL eligibility includes configuration designations (bank and undeposited),
  as well as account kinds and flags. Later configuration changes preserve history.
- Comparers require complete report key sets and validate value syntax and bounds,
  including identical malformed dumps on both sides.

- Reject duplicate stored JSON keys recursively before SQLite or Serde can
  interpret them differently, including escaped aliases and same-value duplicates.
  Inventory every JSON column in `json_evidence::STORED_JSON_COLUMNS` from the
  actual schema. Apply `parse_unique_json` (or its typed wrapper) to every reader
  and evidence predicate, and `fb_json_unique_keys` at insert/update, seal and open.
  Execute an independent table-driven injection for every column.

## INT-L3a integration lessons

- A table rebuild silently drops sibling triggers even when Git merges cleanly.
  Inventory attached guards and restore their reviewed definitions after the
  rebuild. Require safety guards at open as well as hashing objects that remain.
- A numerically sorted migration union changes ordinal versions. Authenticate
  the complete genuine old schema before upgrading; never relabel a newer table
  definition as an old prefix. Compare every old row/key and all new empty tables.
- Share number reservations and settlements through explicit provider views for
  optional modules. Preserve family-specific validation order and messages, and
  test record-time collisions after successful reviews and later void releases.
- Intermediate Python dumps must copy mutable export structures. Validate every
  checkpoint and its financial aggregates independently, including overflow.

## INT-L3b integration lessons

- Freeze the reference Python Unicode version as well as date grammars. A newer
  Unicode crate can silently permit characters the answer key still rejects.
- When a new shared guard intercepts an older independent regression, isolate
  the older guard in the test and restore the exact definition before reopen.
  Keep direct tests for the shared boundary too.
- A JSON-column injection must keep every other JSON column in that row valid;
  otherwise another NULL/invalid field can conceal a missing target predicate.

## INT-L3-FIX1 review lessons

- Commit the installed trigger and required-guard inventories from every reviewed
  branch head and compare their union with the integrated schema. Required-name
  checks alone cannot find a reviewed trigger that was never registered.
- Negative reopen tests first prove a clean reopen with the exact creation key
  and target path, then require the intended integrity diagnostic. Upgrade
  fixtures must follow the upgraded path and compare complete reopened snapshots.
- Differential agreement does not establish that a scenario step worked. Expect
  success by default, fail unexpected outcomes independently on both sides, and
  pin intended refusal kinds and messages. Use the owning document lifecycle.

## JSON dependency feature regression gate

Run `bash crates/fb-db/check_json_features.sh` before integrating JSON parser or
dependency feature changes. It runs the whole workspace, standalone fb-db, and
fb-db plus SimpleFIN with serde_json `arbitrary_precision` and `preserve_order`
explicitly unified. A standalone crate test does not cover workspace features.
`parse_unique_json` delegates value construction to serde_json and checks keys
in a separate pass; custom visitors must never reconstruct arbitrary-precision
numbers from their synthetic map representation. Provider decimal wire tokens
belong in a local `RawValue` decoder rather than a workspace-wide feature.
