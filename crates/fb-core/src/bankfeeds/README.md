# Bank feeds (core half)

Translated from the frozen `bankfeeds/models.py` and `services.py` (tag `python-reference-20261030`).
The provider half (token claim, `/accounts`, parsing, pinning) is the `simplefin` plugin on `main`;
core never depends on a plugin crate and has no network code.

## Entry points

| Python | Rust |
| --- | --- |
| `connect` (after the claim) | `connect(book, actor, co, label, host, &SecretSet)`: registers the connection and pinned host; the secret is a plugin credential |
| host credential load on book open | `restore_connection_credentials`: validate, retain book-wide mask, flag rejected stored connections for reconnect |
| `forget` | `forget` (the host also wipes the plugin credential) |
| `map_account`, `mappable_accounts` | same names |
| `_sync_locked` preflight + `_window` | `begin_sync(.., now, min_interval_secs, force) -> start epoch` |
| `_record_sync(ok=False)` | `record_sync_failure(.., &SecretSet)` |
| `_store` | `store_pull(.., &StagedPull, now, scheduled, window_start, &SecretSet)` |
| `CoreBridge::stage_bank_transactions` | `stage_bank_transactions(.., &[(account_remote_id, StagedTransaction)], &SecretSet)` |
| `match_candidates`, `receipt_candidates`, `rule_for`, `suggest`, `active_rules`, `rule_accounts` | same names (`suggest_for` takes an item of any company, as Python's `suggest(company, item)`) |
| `accept`, `default_check_number`, `ignore`, `return_to_review` | same names; `accept` takes Python's keyword arguments as a JSON object |
| `save_rule`, `move_rule`, `set_rule_active` | same names |

Every write path (`connect`, `forget`, `map_account`, `record_sync_failure`, `store_pull`,
`stage_bank_transactions`, `accept`, `ignore`, `return_to_review`, `save_rule`, `move_rule`,
`set_rule_active`) takes the connection's `SecretSet` as its last argument, which the plugin builds
from its stored credential for the call. Every spelling of those values is masked where provider text
enters and again in every text at the moment it is written (`write.rs`, FIX8), so text composed from
several fields is masked too (threat model and plugin contract: `L4-BANKFEEDS.md`, "Threat model for
credentials").

`stage_bank_transactions` is the scoped bridge's legacy complete-only path: it stages into feed accounts the
connection already downloaded, dedupes on (account, transaction id), never withdraws pending rows and
records no sync; it audits the batch (`bankfeed.staged`: counts and every account / transaction id) in
the same writer. The rich path (`fetch_result` with inventory) maps onto `store_pull`, which alone withdraws
absent pending rows. The host adapter converting `fb_plugin_api::FeedTransaction` (Date -> `YYYY-MM-DD`,
`Amount.cents`) is wave-4 wiring: `fb-plugin-api` is on `main`, not in this `int-l3` base.

## Schema (migration 0133)

Four STRICT WITHOUT ROWID tables plus a forward-only counter. Guards: all-key INSERT and displaced-row
UPDATE collision checks, permanent primary keys and identities, delete protection (only a withdrawn
pending row still to review can be removed), mapping eligibility (company, ASSET/LIABILITY, active,
postable, not a configured control account, currency, one active feed per ledger account), rule account
eligibility and the 200-rule cap, decided values frozen, return-count and status transitions, an accepted
row returns to review only after its entry was reversed, and the decision guard (posted entry of the
company, line on the mapped account in this direction and amount, kind matches the source document kind,
active member as decider). Referenced accounts, users, companies, journals and lines cannot be deleted.
`bankfeeds_invalid` is checked at open; every trigger is required at open.

Migration 0134 (FIX1): byte-exact text on every column (no NUL, Python `len` bounds, through the
`fb_bankfeeds_text` UDF), canonical UUID keys and references (`fb_bankfeeds_uuid`, reusing
`banking_evidence::canonical_uuid`), Python-exact dates (`fb_bankfeeds_date`, year 1 or later) and
round-trip timestamps in the stored `%Y-%m-%dT%H:%M:%S%.6fZ` form (`fb_bankfeeds_timestamp`), at write and
at open (`bankfeeds_text_invalid`); UDFs in `fb-db/src/bankfeeds_text.rs`. A withdrawn transaction's id is
retired for good (`bankfeeds_retired`, filled by the delete trigger and, on upgrade, from the audit
trail). Decisions follow the service rules: an active member decides (ignore included), a match is within
5 days of the item's date, and no new decision names a reversed entry. Reactivating a mapped feed is
checked like a mapping, and open refuses two active feeds on one ledger account. Open-time checks keep
only ownership for mapped accounts: as in Python, an unused mapped account may later change kind or be
deactivated through the account services, and the book must stay openable.

Migration 0135 (FIX2): reactivating a mapped feed re-runs the whole mapping predicate (mappable account:
active, postable, ASSET/LIABILITY, not a control account; the company's currency; no other active feed on
that account), so a feed deactivated before its account was reclassified cannot come back. Neither Python
nor the services reactivate a feed (Python's `BankFeedAccount.active` is never written after creation),
so only direct writes meet it; open-time checks are unchanged.

Migration 0136 (FIX5): provider identity apart from display. Each provider identifier carries an identity
key, HMAC-SHA256 of the raw value under a per-book random secret (`bankfeeds_identity_secret`, made on
first use): `conn_key_identity`, `remote_identity`. Unique keys, collision and fixed-identity guards,
dedupe, withdrawal and lookups use the keys; `conn_key` / `remote_id` hold the redacted text for display
and audit, and may repeat. The two tables are rebuilt (their UNIQUE keys were on the text) and existing
rows get keys derived from their stored text. See `L4-BANKFEEDS.md` "Provider identity kept apart from
display".

## Platform changes (not oddities)

- No `access_cipher`: the access URL is a plugin credential (PLUGINS.md 5). `connected` = `active`.
- Python's integer pk is the book-wide `ordinal` (never reused). `EFT-<pk>` and the uuid5 request keys use it.
  Request-key intents embed UUID account/vendor ids where Python embeds integers.
- `reference_url` holds an app route name (`checks:detail:<id>`), not a Django URL.
- The advisory sync lock is the host's per-connection mutex (single process).
- Dates arrive as local accounting dates from the plugin (it converts epochs with the book's zone).
- Acceptance records and decides in ONE writer for every kind, through the modules' `_tx` forms
  (`checks::review_check_tx` / `record_check_tx` / `get_check_tx`, `banking::review_deposit_tx` /
  `record_deposit_tx`, `transfers::review_transfer_tx` / `record_transfer_tx`, ledger `save_draft_tx` /
  `post_journal_tx`). A check's draft commits first in its own transaction, as Python's `_check_draft`
  does; a retry reuses it through the item's request key.

## Python oddities copied

- `ignore`/`return_to_review` audit `date` as the text `"None"` for an undated item (`str(None)`).
- `begin_sync`'s retry minutes are `floor(remaining/60)+1`, so exactly 330 min left reads "331 min".
- `save_rule` uses Python truthiness for `active` (`"false"` is active).
- `rule_for` lowercases with Python 3.13's `str.lower()` (Unicode 15.1, final sigma) on both sides of
  `in`; `redact` and `default_check_number` use Python `re`'s `\b`, `\s`, `\d` and IGNORECASE folds. The
  tables are generated from the frozen interpreter (`fb-harness/py/gen_bankfeeds_unicode.py` ->
  `py_unicode.rs`), and `testdata/python-*.json` holds about 16,000 differential vectors from the frozen
  `client.redact` / `sanitize`, `default_check_number` and `str.lower` (`gen_bankfeeds_text_vectors.py`).
- Not copied (lead decision, FIX2): Python's `redact` keeps credentials in a URL glued to a preceding
  word character (`xhttps://u:p@h`: `\b` fails, so `_URL` never matches) and in non-http schemes. v2
  removes the userinfo of every `scheme://…@`; see `L4-BANKFEEDS.md` "Python bugs not copied".
- Not copied (FIX4): Python stores provider names, ids, descriptions and messages as its parser kept
  them, so a credential its `redact` misses reaches the rows and later audits (`bankfeed.mapped`). v2
  redacts every provider-derived string once where it enters the core (`staging::provider_text` in
  `store_pull`, `stage_bank_transactions`, `record_sync_failure` and `connect`).
  Identity is not taken from that text (FIX5): rows are deduplicated, matched and withdrawn by the
  identity key of the raw id, so ids that differ only in userinfo stay distinct rows.
- `store_pull` refuses the whole pull when it lists the same NEW transaction id twice for one account
  ("Bank feed transaction with this Feed account and Remote id already exists."): Python reads existing rows
  before its loop, inserts the id twice and `full_clean` refuses. A repeated id that already existed updates once.
  Found by the differential random run; test `a_new_id_listed_twice_in_one_pull_refuses_the_store_like_python`.

## Not copied

- Python's `_store` creates a new feed account with `get_or_create` before `full_clean`, so an account name
  over 120 characters raises a database DataError (an uncaught crash). v2 refuses it with the max-length
  message. The plugin's parser bounds names to 120, so the public path never sends one.

- Postgres sequence gaps after a rolled-back store are not reproduced (the counter rolls back too).
- Python's `_store` itself does not check that the connection is still active (only `sync` does), so a
  direct call could add rows to a forgotten connection. v2's SQL guards refuse new accounts/transactions
  on an inactive connection; Python's public `sync` path never reaches that state.

## FIX10: admission and book-wide final text policy

Public credential constructors enforce the 12-character minimum and the alphabet documented in
`L4-BANKFEEDS.md`. The host must restore all stored credentials through `restore_connection_credentials`
before commands or network use on every open. Rejected existing credentials retain their connection and
history, gain the durable `needs_reconnect` flag (migration 0137), and cannot be synced or staged.
Every bank-feed command adds its mask to `Book::retain_text_guard`; all later `Book::write` calls inherit
the final serialized TEXT policy, including ordinary voids, revisions, reversals and edits. Forgetting
does not remove an in-memory mask before the book closes. Internal legacy masking constructors are
module-private; they support old stored credentials and regression vectors, not credential admission.

INT-L4b enforces the host boundary in the core: opening a book records every active connection whose
mask must be restored. `Book::write` refuses before entering any callback until all those connections
have been restored, even if a later bank-feed call supplies a SecretSet for just one connection.
Forgotten connections have no plugin credential to reload. Reads, reports and integrity checks remain
available. Production restore-on-open wiring remains wave 4; the differential backup host restores
public synthetic fixture credentials only.
