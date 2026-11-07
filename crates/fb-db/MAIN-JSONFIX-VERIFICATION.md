# MAIN-JSONFIX verification

Base: local main `c50025286cd1e3d79f527458bf090c64698d16df`.
Branch/worktree: `main-jsonfix`, `wt-v2/main-jsonfix`. Stop before merge; no push.

## Root cause and changes

SimpleFIN enabled serde_json `arbitrary_precision` for the whole linked workspace.
With that feature, `deserialize_any` can expose a decimal as a synthetic map with
`$serde_json::private::Number`; the old stored-evidence visitor reconstructed that
map as a real object. It also exposed lexical number spellings to shared serializers.
The existing regression failed before the change when run with that feature.

`parse_unique_json` now obtains its returned Value directly from serde_json and
independently checks decoded keys recursively. The checker discards values, including
synthetic number maps. Recursive/escaped duplicates, trailing data, empty input and
malformed JSON remain rejected. Tests also cover native number serialization,
typed reads and the SQL predicate.

SimpleFIN now enables only `raw_value`. Its HTTP decoder preserves raw numeric
balance, available-balance and transaction amount tokens as strings before any
generic Value conversion. Strings and numbers use the existing exact decimal
rounding/limit logic, retained because the Python contract includes coefficients
and exponents beyond rust_decimal's range. The prior large-number test now enters
through this wire boundary, retaining all expected cents. New parser and fake-HTTP
tests cover both forms, extreme precision, exponent forms, invalid types and bounds.

`check_json_features.sh` runs workspace, standalone and explicitly unified-feature
tests. GUARD-PATTERNS documents this regression gate. Production cargo tree confirms
`arbitrary_precision` is absent.

## Grep audit

Command: `grep -RInE 'Visitor|deserialize_any|Number::|serde_json::Number|Value::Number|Number\(' crates --include='*.rs' --include='Cargo.toml'`.

- The only custom Visitor/deserialize_any implementation is the corrected shared
  parser in `fb-db/src/json_evidence.rs`; no other number-building visitor exists.
- `fb-core/src/py.rs:166` writes Number::to_string in Python-style JSON output.
  Arbitrary precision would preserve spellings such as 3.50 here, so removing that
  production feature is material, beyond fixing the visitor.
- `fb-plugins/simplefin/src/parse.rs` formats Numbers in money, epoch and text.
  The HTTP amount path now bypasses float/Number formatting with local RawValue.
  Generic Value callers retain only whatever precision their Value already has;
  numeric identifier text still follows serde_json's configured formatting.
- `fb-core/src/calculations.rs:19` and `fb-db/src/sales/calculations.rs:16`
  format only integer-tagged Numbers. Their amount-input helpers and
  `fb-core/src/services.rs` distinguish integers from floats and use native accessors.
- Payables, checks and prepayments insertion helpers use checked as_i64 conversions;
  sales-tax truthiness uses as_f64. Remaining matches are test fixtures/mutations.
  These inherit native number classification/range behavior; none reconstructs the
  synthetic map. Future global feature changes still require this audit/gate.

## Gates

All completed commands ran in the foreground and were awaited. The initial
disk-backed workspace run was stopped for excessive fixture I/O, with no assertion
failure observed; its log is retained. The complete final test gates used a dedicated
`TMPDIR=/dev/shm/fbv2-main-jsonfix`; no assertions or test coverage were reduced.

```
RUST_TEST_THREADS=4 cargo test --workspace --no-fail-fast
848 passed; 0 failed; 1 ignored
cargo test -p fb-db
36 passed; 0 failed; 1 ignored
cargo test -p fb-db -p fb-plugin-simplefin --no-fail-fast --features serde_json/arbitrary_precision,serde_json/preserve_order
97 passed; 0 failed; 1 ignored
cargo clippy --workspace --all-targets -- -D warnings
exit 0; 0 warnings
FB_HARNESS_TEMPLATE=fbv2_core_jfref FB_HARNESS_DB=fbv2_core_jf bash crates/fb-harness/run_harness.sh
84/84 MATCH; 0 failures; 0 warnings; 3 accepted collation ties
```

Test totals include doctests. The single ignored test is the existing optional
glibc diagnostic. SQLCipher's expected wrong-key diagnostics accompany a passing
negative test. Harness variable files were isolated in `target/harness-var`.
Both job harness databases were dropped and their absence verified afterward.
Formatting, shell syntax and git diff whitespace checks passed.

Full logs, feature tree, grep inventory and per-scenario comparer output:
`/mnt/Data/Projects/FreedomBooks/runs/2026-10-31/main-jsonfix`.

## Left and suggested lessons

Independent review and lead merge remain; main was not modified. No UI changes,
preview or screenshots apply. Keep the unified-feature gate for JSON/dependency
changes; preserve provider decimals locally; choose a dedicated RAM-backed test
temporary directory when repeated encrypted schema fixtures are disk-bound and
memory is available. No skill file was changed.
