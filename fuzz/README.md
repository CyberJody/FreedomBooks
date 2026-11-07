# FreedomBooks v2 fuzz targets

`cargo-fuzz` (libFuzzer) targets for every parser or decoder of untrusted input in the workspace.
This directory is its own Cargo workspace (see `[workspace]` in `Cargo.toml`) and pins the nightly
toolchain in `rust-toolchain.toml`, so the product workspace is untouched.

```sh
cargo install cargo-fuzz
cd fuzz
cargo fuzz list
mkdir -p corpus/money_text && cp seeds/money_text/* corpus/money_text/
cargo fuzz run money_text corpus/money_text -- -max_total_time=600 -timeout=10 -rss_limit_mb=2048
./run_all.sh 600 3        # every target, 10 minutes each, three at a time; logs in fuzz/logs/
```

Every target asserts three things through libFuzzer itself: no panic (ASan + debug assertions),
no hang (`-timeout`; a slow input is written to `artifacts/<target>/timeout-*`), bounded memory
(`-rss_limit_mb`; an input that blows the limit is written as `oom-*`). Most targets add their own
invariants (round trips, bounds, idempotence) as `assert!`s in the harness.

| target | what it feeds | parser(s) under test |
|---|---|---|
| `money_text` | arbitrary text | `fb_money::{parse_python_decimal, money}` (Text, Decimal and Float paths), `Money` print/parse round trip |
| `py_dates` | arbitrary text | `fb_core::py::{model_date, form_date}`, `fb_db::dates::parse_iso_date`, `fb_db::validators::{date, utc_timestamp*}`, `banking_evidence::calendar_date`, `bankfeeds_text::timestamp` |
| `py_int_uuid` | arbitrary text | `fb_core::py::{py_int, py_uuid, py_strip, py_truncate}`, `fb_templates::forms::python_integer`, `fb_ids::parse_uuid`, `CompanyId::parse`, `fb_db::check_numbers`, `prepayments::validate_prepayment_number`, `email_validation::valid_email`, `unicode_decimal`, `char_allowed` |
| `calculations` | arbitrary bytes as JSON (or as a number string) | `fb_core::calculations::{decimal, quantity, unit_rate, line_amount, totals}`, `services::{money_json, amount_input}`, `fb_db::{prepayments::prepayment_cents, banking_evidence::money, assistant::scaled}`; checks the totals identities |
| `simplefin_account_set` | arbitrary bytes as the `/accounts` body | `fb_plugin_simplefin::parse::parse_account_set_json` (RawValue wire decoding, `parse_account_set`, `errors_from`), `feed_accounts`, `feed_transactions` in four time zones |
| `simplefin_scalars` | arbitrary text / JSON scalar | `parse::{money, epoch, redact, sanitize}`; redact idempotence, sanitize limit |
| `simplefin_setup_token` | arbitrary text | `client::decode_setup_token` (base64 variants + URL policy), `HttpPolicy::{new, validate_url, validate_endpoint}`, `address_blocked`, `oauth::{pkce_challenge, verify_pkce, OAuthEndpoints::new}` |
| `openai_receipt` | arbitrary text | `fb_plugin_openai::receipts::parse_reply` (plugin side) and `fb_core::assistant::receipts::parse_reply` (core side) |
| `app_command_json` | arbitrary text | `fb_app::commands::{parse_command, parse_query}`, `fb_db::parse_unique_json`, `keys::WrappedKey` JSON + `keys::unwrap` |
| `app_query_string` | arbitrary text | `fb_app::screens::{parse_query, urlencode, resolve}`, `keys::{from_hex, master_from_hex, check_password}`, `fb_templates::django::{reverse, default_datetime}` |
| `app_form` | structured (`Arbitrary`): a route index, query pairs, form fields, free text | one live encrypted book; `screens::render` (GET with query), `screens::submit` (form POST: every `clean_*` decoder in screens/d2..d8), `App::run` / `App::query` on parsed JSON; the book's integrity check after every iteration |
| `attachments_inspect` | arbitrary bytes + a selected file name | `fb_core::attachments::inspect` (`sniff::{sniff, problem, clean_name, display_name, refusal, invalid_message}`: PDF/PNG/JPEG/WebP container walks), `storage::valid_name`, `Limits::validate` |
| `bankfeeds_text` | arbitrary text | `fb_core::bankfeeds::text::{redact, sanitize, strip_url_userinfo, has_url_userinfo, check_number_in, py_lower}`, `SecretSet::{new, from_credentials, from_access_url, mask}`; run with a short timeout because the inventory flagged super-quadratic scans |
| `fbdb_validators` | arbitrary text / JSON | `fb_db::{validators, checks_snapshots, banking_evidence::payload, credits::fields, prepayments::prepayment_snapshot_ok, assistant::{choices, selection, result}, bankfeeds_text::{text, identity_form, identity_key}, collation::{compare, punctuation_tie, lower, upper}}`, `fb_core::reports::params::{csv_text, page_number}` |
| `backup_archive` | raw bytes as a `.fbbackup` tar, or (odd first byte) a fuzzer-written manifest JSON + book bytes wrapped in a correctly encrypted local backup | `fb_core::backups::verify_backup` (tar entry walk, header.json, manifest decrypt/deserialize/validate, SHA-256 gates, SQLCipher snapshot inspection), `recovery::WrappedKey::{read, unlock}`, `retention::filename`, `crypto::check_new_passphrase` |
| `oauth_callback` | arbitrary bytes as the HTTP request a browser sends to the loopback listener | `fb_plugin_api::oauth::OAuthLoopback::wait` (private `read_callback`: request line, Host header, `state`/`code`/`error`/`client_id` query) over a real 127.0.0.1 socket |

`seeds/<target>/` holds hand-written seeds (valid and edge-case inputs); `run_all.sh` copies them
into the git-ignored live corpus `corpus/<target>/` so each run starts from the interesting part of
the input space. `artifacts/<target>/` is where libFuzzer writes crashing,
hanging and memory-blowing inputs; a crash is reproduced with
`cargo fuzz run <target> artifacts/<target>/<file>`.

`rerun.sh` repeats the targets whose harness assertions were narrowed after a first finding.
Run times and results of the audit run are in `../FUZZ-AUDIT-REPORT.md`.
