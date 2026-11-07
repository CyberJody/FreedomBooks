# L1-FIX1 verification (2026-10-30)

Scope: all ten findings in the independent review of `92985af` on `l1-ledger`.
The review was read from the longest assistant message in state.db session
`20261030_162832_db08bd`, as required. No merge or push is part of this job.

## Failing-first evidence

Before implementation, the added regressions were executed against the reviewed code:

- `review_regressions`: 1 passed, 9 FAILED. Failures included accepted company/account/user
  replacements and delete/reinsert, disabled-PRAGMA writes, missing parents, same-name no-op
  schema definitions, omitted total/retry checks, unsupported dates, unbounded revisions,
  and the autocommit helper leaving a committed journal after an injected audit failure.
- Python-generated money vectors: FAILED with the reported index-out-of-bounds panic.
- Extreme-exponent property: FAILED, shrinking to zero with exponent
  `-18446744073709551618`; the saved proptest seed is checked in.
- Comparer regressions: 5 passed, 5 FAILED (extra/duplicate rows, extra company/date reports,
  ordinary display-looking memos, duplicate natural keys and outcome identity).
- Full-audit runner regression: FAILED because the runner emitted action counts, not events.
- Sample verifier regression: FAILED because the query did not exclude drafts or scope dates/company.
- Additional Python date oracle cases exposed Unicode-year and internal Python-whitespace
  differences before their corresponding fixes; those failing outputs are retained too.

The old autocommit call was changed to a Writer call after its runtime failure was recorded;
compile-fail doctests now prove that Connection cannot call any of the three public `_tx` helpers.

Raw red/green logs and machine-counted results are retained at:
`/mnt/Data/Projects/FreedomBooks/runs/2026-10-30/l1-fix1-evidence/`.

## Findings and proving tests

1. **Money exponent panic/truncation — fixed.** Checked exponent arithmetic, conversions and
   indexing; Python/libmpdec constructor bounds apply even to zero; significant-digit magnitude
   is checked before narrowing. The actual frozen `money()` generated 147 committed vectors,
   including both reported inputs and positive/negative zero boundary cases.
   Tests: `every_python_vector_matches`,
   `review_extreme_exponents_never_panic_and_match_python_limits`,
   `arbitrary_monetary_text_never_panics`, `arbitrary_exponent_narrowing_never_panics`.
   The two inputs and rejected extreme-zero values are also in `edge-half-cent` (now 51 steps).

2. **REPLACE/delete-reinsert bypass — fixed.** Numbered migration 0002 adds collision guards
   covering primary AND alternate unique keys, plus immediate referenced-row deletion guards.
   Protected companies, accounts, operators, journals, posted lines, audit events, seals and
   obligations are attacked with REPLACE and explicit delete/reinsert.
   Tests: `review_replace_and_delete_reinsert_cannot_rewrite_protected_rows`,
   `every_alternate_unique_collision_is_refused_without_delete_trigger_help`.

3. **recursive_triggers OFF history/audit rewrite — fixed.** Collision guards work independently
   of implicit replacement DELETE triggers. Book::write verifies required PRAGMAs before its
   callback and before COMMIT, refusing unsafe state. Book raw connection access is test-only.
   Tests: the replacement tests above, `review_every_write_refuses_disabled_required_pragmas`,
   `write_verifies_pragmas_before_and_after_the_callback`,
   `trigger_creation_order_does_not_change_protections`.
   An external normal-dependency cargo check independently confirms E0599 (raw Book access absent),
   E0603 (unchecked store writer private) and E0308 (Connection is not Writer).

4. **Autocommit multi-row helpers — fixed.** Only Book::write constructs the IMMEDIATE Writer
   capability. Mutating store helpers and journal-number allocation are crate-private and typed;
   the three composable public journal helpers and audit also require Writer.
   Tests: `review_autocommit_helper_does_not_partially_commit`,
   `public_commands_rollback_on_audit_failure`, `typed_public_tx_helpers_rollback_with_their_writer`,
   `all_public_user_writes_rollback_on_injected_late_failure`,
   `reversal_rolls_back_even_when_only_its_final_audit_fails`, and four compile-fail doctests.
   These cover company creation/edit/reminders, account create/edit, draft create/edit, posting,
   reversal, audit, operator create/active update and membership writes, plus deferred COMMIT rollback.

5. **Deferred missing-parent eligibility — fixed in v2.** Line INSERT/UPDATE require existing,
   eligible parents. Posting's LEFT JOIN explicitly refuses a missing account.
   Tests: `review_missing_parents_rejected_at_line_insert_and_posting`,
   `missing_parent_ordering_cannot_commit_ineligible_or_foreign_accounts`.
   **This is an inherited Python raw-SQL bug, not a Rust-only regression.** The frozen Python
   app was not changed. Normal service-level behavior remains the answer-key contract.

6. **Incomplete open checks — fixed.** SHA-256 compares all non-internal sqlite_master SQL
   definitions against schemas built from numbered migrations, including tables/CHECKs, indexes,
   triggers and views. Prior-version definitions are checked BEFORE migration, so migration
   cannot erase evidence of old trigger tampering. Added total and retry/revision violation checks.
   Tests: `review_same_name_noop_trigger_is_detected`,
   `review_open_detects_over_limit_total_and_bad_retry_evidence`,
   `review_retry_evidence_is_checked_on_open`,
   `changed_trigger_index_check_and_unversioned_view_are_refused_on_open`,
   `legacy_schema_migrates_but_tampered_definitions_are_not_repaired_silently`.

7. **Comparer hides corruption — fixed.** Duplicate natural keys are errors; report company/date
   sets and row multiplicity are exact. Only the generated prefix of a linked reversal is
   normalized; ordinary memo text and reason suffixes remain exact. Both runners emit full
   audit actor, actor-name snapshot, target and normalized details, compared in creation order.
   Generated event IDs/timestamps are omitted, not financial or user-entered content.
   Tests: all eleven `fb-harness/tests/comparer.rs` tests, including the review's reproduced
   extra-row, extra-report and ordinary-memo corruptions and audit actor/target/detail mutations.

8. **Sample verifier floats/drafts — fixed.** Decimal cents; posted-only CASE aggregate;
   explicit sample-company identity and as-of date.
   Tests: `test_exact_cents_and_posted_company_date_scope`,
   `test_actual_postgres_drafts_future_and_other_company_are_excluded` (real PostgreSQL TEMP
   tables, maximum-cent value, draft/future/other-company lines). Seeded original sample
   cross-check: `accounts 24 differences none`.

9. **Date-format narrowing — fixed.** Model ISO week/basic dates and Unicode regex fallback;
   all eight abbreviated/full month-name form formats; Python year/day/whitespace semantics.
   Tests: `review_date_formats_match_python`, `accepted_dates_and_errors_match_python`;
   108 model/form results generated by the actual frozen Django DateFields.

10. **Revision overflow — fixed.** checked_add and a controlled error; persisted revisions are
    guarded in 1..=i64::MAX-1 and checked on open.
    Tests: `review_revision_max_is_rejected_without_panic`,
    `revision_overflow_from_a_corrupt_book_is_a_controlled_error`.

## Final gates

- `cargo test --workspace`: 90 runtime tests + 4 compile-fail doctests = 94 passed; zero failures.
- `cargo clippy -p fb-money -p fb-ids -p fb-db -p fb-core -p fb-harness --all-targets -- -D warnings`: clean.
- Production/default-feature core and harness compile without test-only raw connections.
- `cargo test --release -p fb-money`: 5 passed, including all Python vectors.
- `crates/fb-harness/run_harness.sh`: 8/8 MATCH; 2,746 full audit events compared; zero failures;
  only the three known multi-company collation warnings remain (L2a scope).
- Supplemental Python verifier: 2 tests passed, including real PostgreSQL filtering.
- Original seeded sample versus Rust: all 24 account balances match.
- `git diff --check`: clean. Cargo.lock is deliberately not committed.

## Boundary / handoff

`GUARD-PATTERNS.md` is the reusable L2a-and-later contract. L2a must adopt Writer and Book::read,
extend referenced-row guards for its new links, and include its migrations in versioned schema
hash expectations. Source-document ownership, control accounts and cleared-bank restrictions
remain later-layer work, not new unfinished review findings.

Schema hashes detect violations/tampering, not every consistent rewrite by a schema owner
holding the encryption key. They are not signed history, and cannot retroactively prove an old
file was never rewritten. No real data, reference code changes, Electron work, license change,
merge, remote or publishing operation occurred.
