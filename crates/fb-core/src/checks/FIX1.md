# Checks and transfers review fixes

Base: `05d3052`. Migration `0033_checks_review_fixes` appends to the existing
registry; no prior migration changes. This branch stops before merge.

## Stored snapshots

`fb_db::checks_snapshots` uses typed, fallible serde decoding for complete account,
company, vendor/other-payee and item snapshots. It validates conditional empty
snapshots, required nullable item fields, identity agreement and value domains.
Pure `fb_checks_snapshot_valid` SQL predicates run at insertion, check/transfer
sealing and open-time integrity scans. Service row reads validate snapshots too;
numeric SQL binding returns an error instead of calling `expect("integer")`.

The copied reviewer tests reject malformed JSON shapes and transfer corruption.
The numeric-panic regression now first asserts insertion refusal, then simulates
external corruption with the exact update guard restored: public recording must
return an error without unwinding or changing rows, and check/open must refuse.
Additional tests vary every required snapshot field's presence/type, mismatched
identities, malformed costs and noninteger bindings. Seal tests temporarily omit
the insert guard in test-only fixtures to prove the independent seal guard.

## Retained origins

Following the payables fix, `checks_checkline.origin_position` is nullable for
legacy/fresh lines, unique within a revision, and resolves only against the
immediately preceding revision of the same check. SQL inactive-item and
inactive/nonpostable-account eligibility uses that specific originating line.
The service persists its already-validated draft origins. Recording, numbering
and voiding identify each copied line by its position in the preceding revision.
Open-time checks verify stored provenance. Internal provenance is omitted from
the public differential dump.

Regressions cover adding a new inactive item/account beside a legitimately
retained line, duplicate and nonexistent origins, stale origins after reordering,
repeated draft edits, posting/voiding after reactivation and reopen. The populated
upgrade test now includes the exact pre-0033 prefix, copies only prior columns,
preserves all old rows/keys, and checks both valid and tampered schemas. Historical
fixture DDL runs in an atomic transaction, matching production migration setup.

## Comparer

Each side independently requires legal states/actions and transitions, real
calendar dates, flags, bounded integer money/quantities, exact line arithmetic,
conditional snapshot keys/identities, nonblank reference identities, unique
document/revision and revision/position keys, complete revision histories,
current-root agreement and contiguous nonempty line evidence totaling each
revision. Transfers and their voids receive value, snapshot, parent and unique-key
checks too. Tests cover identical malformed pairs and corruption on either side.

## Shared number validator for credits and prepayments

Use `fb_db::check_numbers::normalize(&str)` for service input, mapping its error
into the module's validation error. Use `fb_db::check_numbers::valid(&str)` when
the value must already be normalized, and SQL `fb_check_number_valid(value)` in
insert/open guards. Empty is valid syntax for print-later; each caller must apply
its own required/optional rule. The implementation lives in
`crates/fb-db/src/check_numbers.rs` and preserves the original checks service's
20-character limit, ASCII-edge trimming, and Unicode category exclusions.

The SQL function is connection-free, deterministic and innocuous, registered
before migrations and authorization. Checks use it for root insert/update,
revision insert, sealing and open-time scans. Tests cover the four reviewed
characters U+200B, U+E000, U+2028 and U+0001, callback access, and accepted Unicode
and numeric boundaries. Credits/prepayments integration remains with their owners.

## Review lessons

- Exercise malformed stored scalars through the next public lifecycle operation,
  with explicit no-unwind and unchanged-book assertions.
- Share pure field predicates across service, SQL and open-time boundaries.
- Keep unrelated snapshots valid in direct-SQL guard tests, so an earlier shape
  failure cannot mask the eligibility or lifecycle guard being tested.
- Retention provenance must be unique and relative to the immediately preceding
  revision; new positions and any-match searches are insufficient.

Command logs are in `runs/2026-10-30/l3b-fix1-*.log` under the FreedomBooks project
root. The worker's final report records the final gate counts and commit.
