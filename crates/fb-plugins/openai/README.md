# fb-plugin-openai

First-party, compiled-in `AiHelper` provider for FreedomBooks v2. This crate is the
outside half of the frozen Python `assistant` port, not an accounting engine.
There is no dependency on the ledger, database, money, ids or harness crates.

## What it does

- Sign in with ChatGPT: first-registration `dynamic_agent_client` and
  `agent_name_hint=FreedomBooks`; subsequent `oaiapp_…` registration reuse with
  a login hint only; stable `urn:uuid:` host identity; the framework's ten-minute,
  127.0.0.1-only, random-port callback with state/PKCE-S256/nonce.
- Verifies signed RS256/ES256 ID tokens through issuer discovery/JWKS, allowed
  algorithms, key id, issuer, audience, expiry, issued-at, subject, and nonce.
  Returning sign-in refuses a different subject until disconnected.
- Binds each authorization to a per-CredentialScope generation. A new sign-in,
  Disconnect or validated API-key replacement invalidates older attempts before
  any token request; the generation is checked under the shared mutex before
  exchange and persistence. Other companies/connections remain independent.
- Deserializes successful token responses directly into secret-owning fields,
  never a general JSON Value; duplicate secret fields and malformed types fail
  closed. Only failure responses are inspected for a minimal OAuth error code.
- Serializes token refresh and calls on the shared plugin instance. Rotating
  refresh tokens are saved before reuse; rejected grants drop access/refresh
  credentials but retain registration for sign-in again; temporary failures keep
  credentials. Lost plan scope prevents receipt transmission, including after
  refresh. A 401 on models/Responses invalidates the connection.
- Checks an API key with one models request, stores it only through the scoped
  credential store, and returns/displays only its last four characters. Stored
  keys are revalidated before use; malformed records return a fixed credential
  error rather than panicking on UTF-8 byte slicing. Its model
  picker uses the host-configured list (Python's API-key behavior); ChatGPT's
  picker filters the account's models/visibility, preserving server order.
- Revokes an OAuth refresh token, then forgets locally even if revocation is not
  confirmed or credentials cannot be decoded. Returns revocation confirmation for
  the host to record. Disconnecting an API key does not revoke that key upstream.
- Sends receipt image/PDF bytes inline to Responses, with `store:false` and
  `stream:true`; PDFs always use `receipt.pdf`, never the user's filename. Parses
  the bounded SSE result; failed, incomplete, refused or truncated results cannot
  produce a draft. It returns only completed output, not a live UI stream.
- `read_receipt` uses the exact Python fixed instruction/schema and preserves
  printed values and mismatch warnings. Amounts are rounded directly from their
  original decimal coefficient/exponent, without a precision-losing Decimal
  conversion, using Python's half-even cent rounding. This includes tiny values like
  `1e-100` (zero cents) and coefficients beyond 28 digits. Range checks happen
  before rounding. Quantities must be exactly four-place representable or use
  Python's fallback; they never round into valid quantities. Purchase tax remains a
  separate expense line. `stage_receipt_draft` creates ONLY a draft through
  CoreBridge, and refuses lines whose quantity × price differs from the printed
  amount. The reading retains the printed amount, product, line order and headers
  for core-owned review. Nothing posts.
- Audits every connect/disconnect/slot/read/stage invocation, success and failure,
  with closed action/purpose/outcome codes and elapsed time. Content/credentials
  are never audit metadata. Byte counters are currently zero (not measured).

## Current integration gates — read before use

W1b-ADOPT uses the callback-issued client id from the framework grant, never an
out-of-band host argument. First registration, signed audience binding,
reauthorization binding and stale-callback rejection run through the real fake
callback. The optional connection lifecycle accepts `{}` for OAuth and reports
scoped Connected/Disconnected status, expiry and typed revocation. API-key
connection remains the explicit `connect_with_key` entry point.

The browser authorization URL never contains `id_token_hint`. Login hints are
zeroized after URL construction. Stray loopback requests with no state or the
wrong path do not end the pending sign-in. Provider serialization is per scope;
the provider-wide map mutex is dropped before waiting or making network calls.

The host must persist an install-wide UUID and supply it to `OpenAi::new`. Keep
one shared plugin instance per registry, and run these synchronous operations off
the UI thread. The host also owns encrypted CredentialStore persistence, enabled /
connected / needs-sign-in state, revocation confirmation, book unlock leases,
chosen model, attachment opt-in and rate limits.

`read_receipt` requires ReceiptConsent and rechecks it immediately after refresh.
The raw AiHelper slot cannot itself ask the framework to recheck opt-in; use the
checked receipt wrapper for production receipt entry points. All callers must
supply only company-authorized file bytes. File sniffing/integrity, request-key
deduplication, vendor/account matching and receipt-to-bill linking remain core.

The current DraftBill shape cannot hold the full reading/review metadata. Host
must persist the returned ReceiptReading and enforce authoritative indexed Save
checks in wave 4. This crate conservatively blocks mismatched staging; it does
not implement the UI's explicit "Keep as entered" override or pretend a corrected
product is what the receipt printed. See FRAMEWORK-REQUESTS.md for the remaining
header fields and live streaming. UsageLimit and bounded request-class policies
are adopted; host ceilings must explicitly allow 180 s / 16 MiB for SSE.

## Consent, scopes and data sent outside the machine

The static `MANIFEST` is the consent inventory. ChatGPT scopes are:

    openid profile email offline_access resource.invoke chatgpt.tokens.use.direct
    resource=https://api.openai.com/v1

Authorization sends application name, client registration id, stable install UUID,
loopback redirect, state, nonce, PKCE challenge and requested scopes. Returning
sign-in sends account login / ID-token hints. Token endpoints receive the code,
PKCE verifier or refresh token. Discovery/JWKS authenticates the identity token.
Email/profile/subject and tokens remain in the scoped encrypted credential store;
they are not bookkeeping data and never go into audit events or connection cursors.

Models requests carry the OpenAI access token or API key. A receipt request sends
only the file, fixed instruction, strict JSON schema and chosen model. It does NOT
send the vendor directory, account chart, balances, real filename or other books.
`store:false` is not a claim of zero provider retention; provider account policies
still apply. Only receipt reading is enabled; report/category purposes are refused.

Production HTTP uses only PluginHttp: exact `auth.openai.com` / `api.openai.com`
hosts, HTTPS/certificate checks, DNS/SSRF protection, no proxies/redirects, bounded
bodies and timeouts. No alternative HTTP client or raw provider socket exists.
Fake endpoints are private `cfg(test)` code, never a production feature.

## Testing (synthetic only)

From the worktree root:

    cargo test -p fb-plugin-openai
    cargo clippy -p fb-plugin-openai --all-targets -- -D warnings
    cargo test -p fb-plugin-api -p fb-plugin-testkit

Tests use fb-plugin-testkit FakeServer, the real framework loopback listener,
FakeCoreBridge, MemoryCredentialStore, MemoryAuditSink and injected clock. The
provider fake signs real synthetic ES256 identity tokens, checks code/redirect/
PKCE/client binding, rotates refresh tokens and simulates provider failures. No
provider network, real account, database, browser sign-in or customer data is used.
The shared framework suite covers callback authority/path/timeout/state/duplicate
rejection and HTTP redirect, size and SSRF protections.

The verifier additionally checks an independently Python-generated RS256 fixture
(public keys and synthetic signed claims only, no private key saved). A parity
test retains the original 97 synthetic receipt cases and adds a separate 47-case
precision-boundary corpus, all with expected output generated by the frozen
Python pure parser (reference 084af74, default ROUND_HALF_EVEN). This includes accepted/refused
inputs, cent rounding, source-line arithmetic, dates, Unicode/NFC/control cleanup,
fallbacks and tax lines. The extra corpus covers both sides of cent ties with
29- to 100-plus-digit coefficients, scientific notation including 1e-100, raw
upper-bound violations and quantity precision loss. It does not run Django or
touch a database.

Decimal syntax is ASCII signed decimal/scientific notation. Exponents outside
i64 and unsupported number syntax are refused with fixed InvalidResponse instead
of approximated; unlike Python Decimal, Unicode digits/underscores are not
accepted. This is stricter input validation, not a changed accepted amount.
`1e-100` is supported exactly, so there is no underflow difference for that case.

Optional fixture regeneration (Python; cryptography needed only for RSA):

    python3 crates/fb-plugins/openai/tests/generate_reference_cases.py /path/to/frozen/assistant/receipts.py
    python3 crates/fb-plugins/openai/tests/generate_oidc_fixture.py

The fixed instruction/schema are copied from that frozen answer key's
`assistant/receipts.py`. Python UI/database-only tests belong to later host wiring.

## Jody's eventual real sign-in

Not part of this job. Wave 4 is Jody-initiated only. For the open-source locally
run flow he needs his own ChatGPT account and consent to use his plan; no developer
client-secret/app registration is required (dynamic registration supplies the
client id). Alternatively he can use his own OpenAI API key. Never paste a real
key into reports, fixtures or chat. Paid/remotely hosted app eligibility is outside
this local OSS flow and needs OpenAI approval, per the recorded SIWC research.
