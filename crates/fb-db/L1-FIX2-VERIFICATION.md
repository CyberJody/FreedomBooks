# L1-FIX2 verification (base 4a59131)

All remaining findings A–E in report-review-l1b are addressed. The review was read
in full from its read-only session record; no additional unresolved finding was
present. External key-holder/schema-owner consistent rewriting is still not
signed-history protection, as documented in GUARD-PATTERNS section 4.

## Findings and regression coverage

- A / UPDATE replacement victims: migration 0003 adds BEFORE UPDATE collision
  checks covering every primary/alternate/partial/composite unique key; unconditional
  append-only UPDATE guards remain. Tests:
  `every_update_unique_key_refuses_displacement_directly_and_through_views`,
  `update_or_replace_target_guard_is_bypassed_with_recursion_off`,
  `standard_upserts_and_view_writes_are_refused`.
  Raw attacks run with recursion ON/OFF, both directly and through INSTEAD OF views,
  and compare unchanged table snapshots. The original used-ASSET register remains
  +10.00, not the reproduced -10.00.
- B / hidden rowids: new migration 0003 atomically copies all ledger data and
  recreates all six UUID-keyed tables as STRICT, WITHOUT ROWID; the other three
  composite-key tables already had no rowid. No old migration was edited. Tests:
  `every_protected_table_has_no_hidden_rowid_alias_to_replace`,
  `rowid_replace_deletes_audit_without_colliding_on_declared_keys`,
  `populated_v2_copy_migration_preserves_posted_history_reversals_members_and_seals`.
  All three hidden-rowid aliases are tested for every table. The populated v2 book
  is compared row-for-row after upgrade and reopened successfully.
- C / callback scope: enabled rusqlite hooks and installed an authorizer covering
  the whole write/read callback. It denies transaction/savepoint control, PRAGMA
  writes, schema changes, attachments and schema-table DML; read callbacks deny all
  DML. The SQL-only Reader/Writer facades remove raw Connection/configuration/hook
  access, including the independently reproduced set_db_config trigger-off bypass.
  Tests:
  `authorizer_denies_each_unsafe_operation_and_preserves_write_atomicity`,
  `read_authorizer_denies_control_and_all_dml`,
  `authorizer_is_active_for_trigger_and_view_bodies`,
  `callback_can_toggle_and_restore_recursive_triggers`,
  `defer_foreign_keys_toggle_can_discharge_seal_obligation`,
  `read_callback_can_disable_query_only_and_write`,
  `writer_callback_can_end_transaction_and_leave_committed_work_on_error`,
  `writable_schema_and_trigger_changes_are_detected_unless_restored`,
  plus Reader/Writer compile-fail tests. Ordinary Err, failed deferred COMMIT and
  panic paths restore authorization before rollback; read panic restores query_only.
- D / schema-name filtering: `NOT GLOB 'sqlite_*'` replaces wildcard-underscore
  LIKE. Tests:
  `exact_literal_internal_prefix_includes_all_near_reserved_names`,
  `schema_hash_like_pattern_ignores_non_internal_trigger`.
  sqliteX, sqlite- and mixed-case names fail both check and reopen.
- E / malformed dumps: validate every dump's collection/object/field/tuple types,
  required keys, report-map coverage and nonempty company date maps before comparer
  indexing. Return a failed Report on invalid input, even if both sides are the
  same malformed dump. Tests:
  `null_or_scalar_company_trial_balance_silently_matches`,
  `malformed_types_missing_keys_and_identical_bad_dumps_fail_without_panics`,
  `no_company_report_or_account_register_can_be_silently_omitted`.

Reviewer test names are intentionally retained for correspondence. Their original
reproduction assertions were reversed: these regression tests now require refusal,
rollback and unchanged history. Original reviewer files were not changed.

## Failing-first evidence

Durable directory:
`/mnt/Data/Projects/FreedomBooks/runs/2026-10-30/l1-fix2-evidence/`

- red-replacements.txt: the original replacement-victim, hidden-rowid, transient
  PRAGMA, early-COMMIT, view and writable-schema regressions fail on the old code.
- red-core.txt: three original extra probes fail (deferred seals, read escape,
  near-reserved schema trigger).
- red-comparer.txt and red-strict-dump.txt: malformed report acceptance / panic.
- red-connection-api.txt and connection-escape-red.rs: disabling triggers through
  the pre-facade Connection API rewrites audits.
- red-read-unwind.txt: the additional read-panic cleanup regression fails before
  its RAII restoration fix.

## Final execution

- `cargo test --workspace`: 122 passed, 0 failed (113 runtime tests and 9
  compile-fail doctests).
- `cargo clippy -p fb-db -p fb-core -p fb-money -p fb-ids -p fb-harness --all-targets -- -D warnings`:
  exit 0, clean.
- `FB_HARNESS_TEMPLATE=fbv2_l1_f2ref FB_HARNESS_DB=fbv2_l1_f2 crates/fb-harness/run_harness.sh`:
  exit 0, 8/8 MATCH; 2,746 complete audit events compared. Only the 3 known
  collation warnings remain (all on edge-multi-company).
- `git diff --check`: clean.
- Dropped the template fbv2_l1_f2ref; retained only this job's final scenario DB
  fbv2_l1_f2. Exact names checked in pg_database; pre-existing jobs' DBs untouched.
- No UI/server changes; preview/screenshots are not applicable to this Rust ledger fix.

Logs: workspace-final.txt, clippy-final.txt, harness-final.txt,
retained-databases.txt and machine-aggregated summary.json in the evidence directory.

## Public API / downstream adoption

See `GUARD-PATTERNS.md`, especially “Changes since 4a59131”, for the complete
required L2a/L3 checklist and attack matrix.

- New types: fb_db::Reader and fb_db::Query (prepare/query_row).
- Book::read callback: &Reader, formerly &rusqlite::Transaction.
- Writer no longer dereferences to Connection; supported SQL methods are inherent.
  No configuration/raw-handle/hook methods are exposed.
- Public store read parameters now take &(impl Query + ?Sized): user, is_member,
  company, get_company, account, accounts, account_has_lines, account_code_taken,
  journal, journal_in, lines.
- Public services read parameters now take &(impl Query + ?Sized): authenticated,
  access, next_entry_number_tx, journal_snapshot.
- Book::write and typed mutating helper signatures are unchanged. Inferred callback
  callers using supported SQL methods do not need edits.
- Duplicate UPDATE-key collisions now return named fb_* guard errors instead of
  unique:* errors. Schema version advances to 3; downstream numbered migrations
  must follow L1's rebuild. Dump extensions must update the strict shape validator.

Stop before merge. No parallel worktrees, reference Python code, Cargo.lock,
production data, remotes or licenses were changed by this commit.
