# FIX3b validator audit

Answer key: frozen `banking/reconcile.py` and `banking/reconcile_views.py` at
`python-reference-20261030`. This audit covers rejection grammar and the order
of validation at every ported reconciliation entry point. State, permission,
retry and accounting behavior remains covered by the existing banking harness.

| Validator / entry point | Rejection branches and ordering checked | Permanent coverage |
| --- | --- | --- |
| `BeginForm` | Required date/ending/key; invalid and out-of-range date; every CharField length; all field errors precede service errors | `dfix3b-begin-statement-date-*`, `begin-length-*`, `begin-form-before-service`, original `d7b-begin-invalid` |
| `_uuid` | Invalid, blank, negative, oversized numeric text; request key before date/amount, completion key before service revision, undo key before revision/reason | `dfix3b-begin-request-key-*`, `begin-key-before-amount`, `finish-key-*`, `undo-key-*`; `reconcile_key_validation_precedes_all_other_fields` |
| `_uuid` accept grammar (FIX3c) | `uuid.UUID` runs `int(hex, 16)` after its 32-character check: `0x`/`0X` (and one `_` after it), `+`, single `_` between digits, surrounding whitespace and Unicode digits are accepted; `__`, trailing/leading `_`, bare `0x` refused | `dfix3c-key-*`; `py::tests::uuid_text_accepts_python_int_grammar`, `uuid_text_matches_python_vectors` (6,070 Python 3.13 vectors) |
| `_date` | Bad/blank date, both financial range ends; view validates date before service invocation | Above date fixtures; existing `begin_invalid_amount_has_exact_field_wording_order_and_no_writes` checks service date-before-amount |
| `parse_amount` | Every amount: malformed, blank, signed, zero, nonfinite, fractional cents, above maximum; ending before charge before interest before revision | `dfix3b-begin-ending-balance-*`, `service-charge-*`, `interest-*`, ordering fixtures; existing amount Rust matrix |
| `_revision` at begin | Optional absent/empty; malformed, negative, above 10^9, decimal text; valid bounds, signed, padded, underscore and Unicode integer text; amounts precede revision and revision precedes account/state | `dfix3b-begin-revision-*`, `begin-valid-revision-*`; `reconcile_raw_revision_grammar_and_order_match_python_without_writes` |
| `_charge_account` | Malformed/blank ID vs numeric nonexistent/ineligible account; charge account before card interest and statement state; interest eligibility before statement state | `dfix3b-begin-service-account-*`, `interest-account-*`, `begin-charge-before-current`, `begin-charge-before-card-interest`, `begin-card-interest`; `optional_charge_accounts_are_validated_before_statement_state` |
| Screen action / shown / ticks | Invalid action; malformed shown pairs and revisions, blank shown accepted, count >900, non-integer/blank/negative/oversized tick, tick outside shown; shown before tick before revision | Original invalid-action/shown; `dfix3b-{save,reconcile}-{shown,tick,revision}-*`, membership fixtures; `shown_parser_checks_size_count_grammar_and_python_decimal_digits` |
| `_revision` at save / finish | Required at both services, before lookups/retries; screen revision before save/completion; accepted bounds remain stale-state checks | Above screen fixtures; core revision tests and `required_revisions_and_keys_precede_reason_lookup_and_retries` |
| `_revision` at undo | Required, exact message, key before revision before reason, 12-character view slice; valid stale value reaches concurrency error | Six `dfix3b-undo-review-*`, original truncated fixture, reason-before-stale and stale-before-lookup; `undo_slices_revision_before_parsing_and_refuses_without_any_writes` plus core tests |
| Undo reason | Blank/non-string, whitespace, >500 characters; before state/revision mismatch after revision grammar | Original invalid undo, `dfix3b-undo-reason-long`, `undo-reason-before-stale`; `undo_reason_matches_python_before_lookup_and_without_writes` |
| `ReportForm` and export cap | Both choices reject malformed, signed/oversized numeric and padded text; invalid choices before export cap; print/CSV >2,000 refused for both detail and summary; OPEN redirect before choice validation | `dfix3b-report-{view,output}-*`, `report-cap-*`, `report-choice-before-cap`; OPEN redirect asserted in `begin_leave_resume_finish_report_and_undo` |

Changes found by this audit: reconcile-specific key messages; required revision
checks in undo, save and finish; Python integer parsing for begin's string
revision; charge-account eligibility checked before statement-state errors;
raw tick shape/integer validation before reconciliation lookup and Python's
acceptance of signed/boolean/integer-text line revisions on no-op saves.
The shown parser also counts Unicode characters and includes the full UUID
and delimiter allowance at 900 rows; its maximum-width ASCII and Unicode
revision cases have direct Rust tests.
Completed-statement POSTs also validate tick membership before the closed-state
branch and completion key. `dfix3b-closed-*` fixtures cover Save/Reconcile,
membership-before-key and revision-before-membership; the end-to-end flow test
asserts that a malformed completed retry is refused without writes.
`dfix3b-screen-record-id-*` independently exercises malformed, blank, negative
and oversized reconciliation IDs with a valid action and other bad fields;
the flow test checks the chooser message and no writes for all four.
All refusal tests snapshot the complete database where they call a service;
the screen comparer also rolls back and rejects POST mutations.

The renderer's existing 10,000-byte field guard precedes the shown parser's
length ceiling. The parser's otherwise unreachable length branch and the
valid 900 / invalid 901 pair boundaries have direct Rust coverage. No transport
limit or renderer security setting was relaxed. The existing deliberate
control/format-character refusal for newly typed free text still applies.
