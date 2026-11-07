# Writing a FreedomBooks plugin

## Boundary and registration

Put first-party crates in `crates/fb-plugins/<provider>`. Depend on `fb-plugin-api`,
never core, DB, SQLite, or another provider. Testkit is a dev-dependency only.
Implement `Plugin` and the appropriate object-safe slot: `DocumentSender`,
`BankFeed`, `AiHelper`, `SalesImporter`, or `RemoteAccess`. Calls are synchronous;
the host runs network work off the UI thread. Headless core uses
`default-features = false`, avoiding reqwest. Use integer-cent amounts, validated
accounting dates, UUIDs, and the injected clock; core validates accounting rules.

Return a static manifest describing auth, consent scopes, settings, data read/sent,
and exact network hosts. Use secret settings fields, never credentials in manifests.
Register typed slot handles for every declared slot. Registration does not enable
connections: persisted company states do. `enabled_connection` permits sign-in and
forget; `connected` requires Connected. AI/remote-access slots are company singletons;
senders/bank feeds may have multiple connections. Share one provider instance.

## Context and core responsibilities

`PluginContext` binds company, connection, actor, credentials, HTTP, audit, clock,
and `CoreBridge`. Bridge operations render documents, stage feeds, create drafts,
attach files, and record sends/state. No SQL or posting operation exists. People
confirm posts. RemoteAccess reports status; it must not start a listener.

Host owns consent, matching, dedupe, posting, polling, timezone, connection
serialization, and transactionally coordinated state/audit/credential writes.
Stage exactly once: host staging OR a provider convenience method, never both.

## Cancellation and unlocked-book lifecycle (W1-FIX1)

Host constructs `OperationLease` per scoped operation and passes it to
`PluginContext::new_cancellable`. Cancel outstanding leases on book lock/close,
disable, Forget, configuration change, or superseding Connect; never reuse a
cancelled token. `check_active` is cooperative, NOT proof of an unlocked book.
Legacy `new` remains compatible but provides no external lease owner.

Credentials and HTTP clones retain cancellation. HTTP checks before DNS/send,
between reads and after completion; blocking I/O is still bounded by its deadline,
not instantly interrupted. Use `wait_cancellable(cx.cancellation())` for OAuth:
waiting and partially received callbacks check cancellation promptly and close.
Providers recheck before token exchange and credential save. Keep provider
mutex/generation protections until host coordination is integrated and verified.

Wave 4 must hold real unlock leases and serialize lock/Forget against service
commits. A pre-call atomic flag cannot eliminate a cancellation-versus-save race;
CoreBridge and encrypted stores must recheck under their transaction/lock. This
framework is not the book-lock implementation. Audit may record cancellation.

## Credentials and lifecycle (W1-FIX1)

Use scoped credential load/save/forget only. Never persist tokens/access URLs in
connection state, preferences, files, or logs. Secret wrappers redact Debug and
zeroize owned memory, not every transient library copy. Return fixed errors, never
provider bodies, URLs, email content, or transport diagnostics.

`EncryptedCredential` and `ConnectionKeyWrapper` define envelopes, not encryption.
Wave 4 supplies random per-connection keys, authenticated scope binding and
`plugin-credentials` HKDF labeling. Forget wipes blob and wrapped key.

Optional `Plugin::connection_lifecycle()` exposes object-safe `ConnectionLifecycle`
and provider-owned `PendingConnection`. Provider validates input and owns the
one-shot attempt; host persists returned scoped `ConnectionUpdate`, validating it
against the active context. Connect returns Connected after credential save;
unrefreshable expiration returns Expired, confirmed upstream invalidation Revoked
(or NeedsSignIn when cause is unknown). Access-token expiry alone is not disconnection:
refresh first. Local Forget returns Disconnected plus `RevocationStatus::Confirmed`,
`Unknown`, or `NotApplicable`. Unknown never means revoked upstream. Registry status
is host-owned; these contracts do not persist or automatically transition it.

## OAuth callback and registration (W1-FIX1)

Use `OAuthEndpoints`, `OAuthConfig`, and `OAuthLoopback::start`. Pass the secret
authorization URL only to the host's system-browser opener. Listener is already
bound to numeric `127.0.0.1:0`, never wildcard/localhost. Default deadline is ten
minutes. `wait` consumes it; dropping closes it. PKCE S256, state and nonce are
fresh. Callback path/authority must match exactly; duplicate parameters fail closed.

`OAuthGrant::returned_client_id` preserves optional client_id from the SAME
state-validated callback. `client_id` remains the requested identifier. Returned
metadata is not authenticated identity: dynamic-registration providers validate
shape and reauthorization binding, then use that validated identifier in token
exchange and signed audience verification. `token_form()` deliberately uses the
requested id; dynamic providers construct their provider-specific form. Verify
OIDC signature/issuer/audience/expiry/claims, then `verify_nonce`, before saving.
Never decode an unverified JWT to discover identity or ask users to copy codes.

Register a native/public desktop loopback flow supporting numeric IPv4 and dynamic
ports with the exact callback path. Provider acceptance is a separate integration
gate: especially Microsoft's personal+organizational registration, do not infer
support from localhost portal examples. Tests prove our bind/redirect contract,
not real provider acceptance. No new listener, fixed-port fallback, relaxed host
check, or real sign-in is authorized here.

## Transport policies and TLS proof (W1-FIX1)

Use the context's `PluginHttp`. Host grants exact manifest/consented hosts and
ceilings. `constrained(timeout, request_bytes, response_bytes)` takes each minimum
and retains the injected resolver, host policy and test seams. `restricted_hosts`
intersects, never expands, allowed hosts. Do not reconstruct clients with `new`
or replace host limits with `with_limits` inside plugins. Set host ceilings high
enough for approved request classes; plugins only tighten. Streaming remains bounded,
buffered response processing, not a live SSE API.

`HttpPolicy::validate_url` is non-I/O preflight, not DNS approval. HTTPS, certificate
verification, no proxies/redirects, all-answer public-address validation and pinning
remain mandatory per request. SMTP uses `validate_endpoint`/`resolve_endpoint`
with the same injected resolver and deadline ceiling, then numeric pinned sockets
and TLS authentication of the original hostname. No provider-owned DNS resolver.

Testkit `FakeTlsServer` and test-only `with_tls_test_route` map an exact validated
synthetic public address to one exact loopback socket AFTER production checks.
They retain URL Host/SNI/name verification and inject an ephemeral trust anchor.
Tests prove one resolution/request, real TLS success, wrong-name/untrusted refusal,
and private/mixed answers refused on subsequent requests before connecting.
Never link testkit or enable `test-support` in production; no insecure TLS option exists.

## Bank results and warnings (W1-FIX1)

Override `BankFeed::list_accounts_result`/`fetch_result` to return `FeedResult`:
items, completeness, account inventory/metadata, and closed warning codes with
normalized identifiers/source indexes. Never copy provider error text into audit.
Legacy Vec methods remain; their default envelopes are Unknown, not Complete.
None inventory means unavailable, not known-empty. Partial pulls retain valid data
and warnings. `has_complete_inventory` is a conservative preliminary guard;
core also checks query scope before pending-withdrawal reconciliation. Keep richer
provider parsing where required; this boundary does not replace core sync logic.

## Mail outcome and typed audit (W1-FIX1)

`send_outcome` distinguishes NotSubmitted, Accepted with receipt and optional local
recording error, and Uncertain. Legacy errors conservatively become Uncertain.
Override it at the provider acceptance point to retain receipts when audit/history
fails. Acceptance (SMTP 250/Graph 202/Gmail response) never proves recipient delivery.
None is a valid provider id. Never auto-retry; inspect provider history when uncertain.

`DocumentSendAudit` extends legacy `AuditEvent` without breaking struct literals:
document ref, receipt, scoped actor/time/counts and closed event codes. Use
`record_document_send_audit`; an unsupported sink fails closed rather than dropping
metadata. Validate opaque provider IDs; no content, addresses or tokens. Testkit
retains typed events and bridge receipts. Wave 4 owns durable pre-network attempts,
crash recovery and atomic history/audit. Agree on single recording ownership when
wiring: Gmail currently records via bridge; never duplicate its event blindly.

## Verification

Use synthetic servers, memory credentials/audit, fake bridge and fixed clock only.
Memory storage is not encrypted persistence. Run both framework package tests and
clippy with `--all-targets -- -D warnings`. See `MIGRATION-W1-FIX1.md` for exact
provider changes and deferred core work. No real accounts, publishing, or license
choice belongs to framework tests.
