# fb-plugin-simplefin

FreedomBooks v2's read-only SimpleFIN `BankFeed` plugin. This is the **outside
half** of the frozen Python `bankfeeds/client.py`, not matching or posting code.
There is no dependency on a ledger/database crate and no database access.

## What it does

- Decodes a Base64 / URL-safe Base64 setup token, accepting pasted whitespace.
- Makes **one empty POST**, without automatic retry, to claim it. Validates that
  the returned credential-bearing access URL keeps the claim's hostname, scheme
  and effective port. Never follows redirects.
- Stores the access URL and its origin pin only through the scoped
  `CredentialStore`. Does not retain the setup token or write a state blob.
- Fetches `/accounts` with Basic auth, optional epoch windows, `pending`,
  `balances-only`, and repeated `account` filters. The slot uses posted-only
  queries by default, exactly as Python does, but still marks a returned row
  pending when `posted` is missing/zero even if the provider says `pending=false`.
- Parses both v1 `org`/`errors` and v2 `connections`/`errlist` Account Sets.
  Preserves account identity, balances, organization, timestamps, warnings and
  skipped reasons in the rich parser result. No missing money/dates are invented.
- Converts money directly from decimal text to integer cents with half-up
  rounding, including exponent notation and negative amounts. Keeps the Python
  pre-round maximum `99999999999999.99`. JSON numbers retain their lexical
  decimal representation; money never passes through a float.
- Converts feed timestamps using the **book's explicitly supplied timezone**,
  including DST. `DateWindow` means inclusive local calendar days; an invalid,
  reversed, unsupported or ambiguous-midnight window is refused, not guessed.
- Returns staged-feed candidates. An explicit convenience method sends them
  through `CoreBridge::stage_bank_transactions`; nothing is posted. The core owns
  dedupe, matching rules, account mapping, withdrawn pending rows and acceptance
  into checks/deposits/transfers.
- Forgets its scoped credential without deleting staged history. SimpleFIN has
  no protocol revocation endpoint: revoke access separately at SimpleFIN.
- Audits calls with closed action/result codes, injected timestamps, elapsed
  duration and completed response body byte counts; never provider contents,
  headers, URLs or bank descriptions.

## Host contract

Construct `SimpleFin::new(book_display_time_zone)` (for example an IANA timezone
stored with the book). The `PluginContext` must be enabled, unlocked, and scoped
with plugin id `simplefin`. Connections are identified only by the context's UUID.
The host must serialize calls per connection, apply polling intervals and obtain
consent; none of those core policies is invented by this provider.

`BankFeed::connect` takes the **raw UTF-8 setup-token bytes** in
`ConnectInput.fields`, not a JSON object. Once credentials exist, reconnect is
refused until the user explicitly forgets the connection. A failed claim/save or
timeout must not trigger a blind retry: the one-time token may already be consumed.

Use one staging path:

1. `BankFeed::fetch_result` returns items plus completeness/warnings/inventory;
   the host handles warnings and stages the valid items once, OR
2. `SimpleFin::fetch_and_stage` fetches and calls the scoped bridge once, returning
   `StagedFeed` identifiers.

Never call both for the same pull. No dedupe is attempted in the plugin; retain
`account_remote_id` + `remote_id` unchanged in meaning for the core's dedupe key.

`BankFeed::list_accounts_result` and `fetch_result` implement the W1-FIX1 rich
slot. Valid items survive partial pulls. Closed warning codes carry normalized
account/transaction/connection IDs and zero-based source indexes (in the relevant
accounts, transactions, errlist or errors array), never provider messages. Account
metadata retains organization ID/name, connection ID, available balance and balance
timestamp. Inventory includes identifiable accounts even if their rows are skipped.
Explicit `accounts: []` is known-empty; absent/non-list accounts is an error, not an
invented empty inventory. Only warning-free pulls are Complete. The host must also
check the pull's account/date/pending scope before using `has_complete_inventory`
for pending withdrawal; a balances-only pull does not inventory transactions.

Legacy `list_accounts`/`fetch` vectors and `fetch_and_stage` still fail closed on
partial data. `gen.auth*` warnings map to AuthenticationRequired in rich results;
legacy methods return NeedsSignIn. Other provider diagnostics map conservatively
to Other, rather than guessing their meaning from text. Skips use the closed
InvalidAccount, InvalidTransaction, MissingIdentifier, UnsupportedCurrency and
InvalidAmount codes.

`SimpleFin::fetch_account_set` accepts `client::FetchOptions` and preserves the
full tolerant protocol parser result, full transaction epochs and sanitized
provider diagnostics without staging. It remains useful beyond the neutral slot;
it is not a workaround to delete. See [FRAMEWORK-REQUESTS.md](FRAMEWORK-REQUESTS.md)
for the remaining wave-4 host obligations.

## Scopes and what leaves the machine

The static `MANIFEST` in `src/lib.rs` is the consent inventory:

- Outside account: the user's own SimpleFIN Bridge connection.
- Authentication: setup token; **no OAuth scopes, client id, app registration or
  banking password** is required by this plugin.
- Permission: read linked accounts, balances and transactions only. No payments,
  transfers or changes at the bank can be initiated.
- Outbound: the claim token in its HTTPS request path, access credentials in the
  Basic Authorization header, query dates, optional remote account identifiers,
  read flags, ordinary HTTP headers and the network source address. No journal,
  invoice, attachment, match rule or local ledger-account data is uploaded.
- Inbound: linked-account metadata, balances, transactions and provider warnings.
- `data_sends = BankData` is conservative: the outgoing account IDs/date windows
  are bank-query metadata, not an upload of the user's books.

The default exact hosts are `bridge.simplefin.org` and `beta-bridge.simplefin.org`.
The official Bridge setup flow and [developer demo](https://beta-bridge.simplefin.org/info/developers)
currently issue claim URLs on the latter. Both retain HTTPS, DNS/private-address
checks and same-origin claim/access pinning. A self-hosted/other SimpleFIN server
requires explicit host consent and a matching context HTTP allow-list;
there is no wildcard or automatic trust of any hostname inside a pasted token.
Never display or log the claimed access URL. Error variants are fixed phrases,
not raw provider responses. Claim HTTP 403 maps to `SetupTokenRefused`; established
feed HTTP 403 remains `NeedsSignIn`. HTTP 402 maps to `ProviderSubscriptionRequired`.

## Network and secret safety

Every request uses the framework's `PluginHttp`: HTTPS/certificate validation,
no environment proxies, no redirects, all DNS answers validated, and connections
pinned to those checked addresses while retaining the URL's hostname for TLS.
Private, link-local/metadata, CGNAT, site-local, reserved, documentation,
benchmarking, Teredo, 6to4 and mapped/NAT64 forms are refused. W1 is intentionally
stricter than Python for *public* transition/mapped IPv6 as well.

Operation caps are 4,096 bytes / 30 seconds for claim and 20 MiB / 90 seconds for
accounts, including streamed responses. Every operation uses the framework's
constrained helper: these ceilings intersect the host ceilings and never enlarge
them. The injected resolver, cancellation and exact host policy are preserved.
A default 30-second host ceiling therefore keeps accounts at 30 seconds; a host
must explicitly grant 90 seconds if it intends to permit the full protocol budget.

Compile-time testkit seams allow numeric-loopback HTTP and exact synthetic-public-IP
to loopback TLS routing after production address checks. TLS retains hostname/SNI
and certificate verification with an ephemeral test trust anchor. No production
feature in this crate enables either seam. Plugin tests prove resolver retention,
16-byte host caps, 25ms host deadlines, real TLS Host/SNI, wrong-name/untrusted
certificate rejection, and later private/mixed DNS refusal before another socket.
FakeTlsServer serves `ok`: TLS tests prove completed transport, then JSON rejection,
not a successful Account Set sync; numeric HTTP fakes test successful protocol flows.

Long-lived credentials use `SecretString`/`SecretBytes`, with zeroizing temporary
Base64 buffers. No secret-bearing type has a plaintext `Debug` implementation.
URL/JSON/HTTP libraries may hold transient copies; zeroizing wrappers are not a
claim of secret-free process memory. The core must supply encrypted persistence
and coordinate store/audit changes; this crate does not implement encryption or
atomic database transactions. The testkit memory store is deliberately not
persistent/encrypted.

## Testing

From the workspace root:

    cargo test -p fb-plugin-simplefin
    cargo clippy -p fb-plugin-simplefin --all-targets -- -D warnings
    cargo fmt -p fb-plugin-simplefin -- --check
    cargo check -p fb-plugin-simplefin --lib
    cargo test -p fb-plugin-api -p fb-plugin-testkit

After dependencies are cached, add `--offline` to Cargo commands as appropriate.
Network tests use only `fb-plugin-testkit` loopback HTTP/TLS fakes and synthetic
credentials. Address tests inject answers or validate policies without opening
public sockets. No SimpleFIN demo/live API call, real bank or live DNS lookup is
part of the tests. Testkit is a dev-dependency only.

Coverage:

- `tests/bank_feed.rs`: claim/fetch/forget/stage through the slot, Basic auth,
  query flags/windows, host/origin pin, malformed replies, error statuses,
  no redirects/retries, both response-cap framing paths, scope, audit,
  credential and bridge failures.
- `tests/hardening.rs`: token parsing; translated `test_fix1.py` and
  `test_hardening2.py` range/address cases, both ends of every Python blocked
  network, mixed DNS answers, obscure numeric hosts, test-seam isolation and
  framework deadlines. No actual public-IP connection is made.
- `tests/framework_fix1.rs`: plugin-path host limit/resolver regressions, TLS/SNI
  and rebinding checks, rich slot completeness/inventory/metadata/warning mapping,
  legacy partial refusal, scope checks and subscription errors.
- `tests/parsing.rs`: exact rounding/bounds, v1/v2 data, pending rows, warnings,
  sanitized text, local accounting dates and 23/25-hour DST windows.
- `tests/python_parity.rs`: committed golden results obtained by executing the
  frozen Python client's pure functions on synthetic inputs: 547 money cases,
  17 epoch cases and 32 Account Sets. Normal Cargo tests do not require Python.

To regenerate those golden cases explicitly (frozen reference read-only):

    python3 crates/fb-plugins/simplefin/tests/python_oracle.py /path/to/frozen/bankfeeds/client.py

The generator stubs only the unused Django settings import, never loads settings
or a company, and performs no DNS, HTTP or database work. Review generated changes
rather than silently blessing changed Rust outputs. The Python `test_http.py`
Django views, advisory-lock races, matching and posting tests belong to the later
host/core wave, not this outside-half crate.

## Jody's later real connection

Only after wave-4 integration/security review and Jody's go: he creates a new,
separate FreedomBooks app connection in his SimpleFIN account and supplies its
fresh one-time setup token. Do not reuse another application's token/access URL.
If SimpleFIN refuses a token as used and he did not use it, disable that connection
at SimpleFIN and create a new one: someone else may have claimed it. No developer
receives or types his bank password. No real sign-in was performed for this job.
