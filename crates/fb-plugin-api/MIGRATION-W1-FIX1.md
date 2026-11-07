# W1-FIX1 migration

Scope: additive framework/testkit changes only. No provider implementation is changed
by this branch. Compiling unchanged proves compatibility, NOT adoption of these
contracts or readiness for live accounts. Existing public structs used by providers
remain constructible. `OAuthGrant` already has private nonce state; its added field
does not break external literals. New error/status variants can require downstream
exhaustive matches to be updated. The five reviewed providers use fallback matches.

## OpenAI

1. In `src/auth.rs::complete_authorization`, remove the externally supplied
   `callback_client_id: Option<&str>` argument. After scope validation, call
   `pending.flow.wait_cancellable(cx.cancellation())` BEFORE selecting the issued
   client id. Read `grant.returned_client_id.as_deref()` instead. Preserve current
   `valid_client` validation (`oaiapp_...`), require it for `dynamic_agent_client`,
   and reject a different returned id on reauthorization. Keep requested id separate.
   Keep explicit provider form construction including resource; `grant.token_form()`
   still deliberately uses the original requested client id. Keep signed ID-token
   audience/issuer/signature/expiry and nonce validation before saving credentials.
   Change fake redirects/tests to send client_id on the real framework callback,
   not pass it through a host argument. No copy/paste code or second listener.
2. In protocol requests, use `cx.http().constrained(30s, input_cap, 2 MiB)` for
   metadata and `constrained(180s, input_cap, 16 MiB)` for SSE. Host must explicitly
   grant a 180s ceiling; a 30s host ceiling stays 30s. Keep 10 MiB input checks and
   bounded completed-response parsing. This is not a live streaming implementation.
3. Map usage-cap 429 to `PluginError::UsageLimit`; keep credentials. No provider
   error strings in errors or audit.
4. Implement optional `ConnectionLifecycle` through `Plugin::connection_lifecycle`
   as an adapter around existing begin/complete/disconnect. Return scoped
   `ConnectionUpdate`; map existing disconnect None/Some(true)/Some(false) to
   NotApplicable/Confirmed/Unknown revocation with Disconnected status. Do not
   discard provider validation or credential serialization locks.
5. Retain host-supplied stable install UUID and ReceiptReading/ReceiptConsent.
   Install settings, authoritative receipt fields/override audit and Save checks
   belong in core wave 4, not connection cursors. Do not delete these workarounds
   until the core artifact/consent contract exists. Multi-auth manifest inventory
   and richer OIDC data labels remain a separate consent-UI design, not invented here.

## SimpleFIN

1. Replace `src/client.rs::execute`'s policy clone/with_limits/new-client sequence
   with `cx.http().constrained(timeout, 1, cap)?.execute(request)`. Keep claim 4096
   bytes/30s and accounts 20 MiB/90s; all are intersections with host ceilings.
   Remove the `PluginHttp` import if unused. This preserves injected DNS and a
   stricter host deadline. Replace the empty-answer/Network-sentinel helper with
   `policy.validate_url(url)`; it does not resolve DNS.
2. Override `BankFeed::list_accounts_result` and `fetch_result` in `src/lib.rs`,
   using the existing `fetch_account_set` parser once per operation. Carry valid
   items even when provider errors/skips exist, mark Partial, map skips/errors to
   closed `FeedWarningCode` plus normalized account/transaction/connection IDs and
   source index. Preserve organization, connection, available balance and balance
   timestamp through `FeedAccountMetadata`; populate present account IDs even for
   accounts whose individual rows were skipped. Known-empty is Some(empty), absent
   inventory is None. Only mark Complete for a verified complete pull. Authentication
   failures with no usable data may still return NeedsSignIn.
3. Keep old Vec methods failing closed for partial data so legacy callers cannot
   silently lose warnings. Switch host integration to rich methods before removing
   that refusal. Do not use default legacy envelopes as completeness evidence:
   they are Unknown. Keep `parse::AccountSet` for protocol-specific normalized
   fields, full transaction epochs and sanitized provider diagnostics not represented
   by this neutral envelope. Do not pass raw provider messages through warnings.
   The parser is not a workaround to delete. Host must check pull scope as well as
   `has_complete_inventory` before withdrawing missing pending items.
4. Map refused setup claims to SetupTokenRefused and subscription 402 to
   ProviderSubscriptionRequired; add provider mapping tests. Do not reinterpret
   authentication failure during an established feed as a used setup token.
5. Replace the stated missing TLS proof with testkit `FakeTlsServer` plus injected
   resolver and `with_tls_test_route`, available only under test-support. Keep
   numeric-loopback transfer-cap tests. No production TLS bypass. Failed-transfer
   byte accounting still lacks consumed-byte metadata; do not report fabricated counts.

## SMTP

1. Change `src/transport.rs` to receive the context HTTP boundary, not just a
   copied `HttpPolicy`. Replace the HTTPS-shaped URL/empty-DNS sentinel and
   `SystemResolver` call with `cx.http().resolve_endpoint(config.host, config.port,
   remaining_attempt_deadline)`. Use only returned numeric addresses and the
   returned remaining budget; TLS still authenticates the original host. Keep
   lettre TLS/STARTTLS, MIME limits, no retries and consent checks. The framework
   helper resolves/pins, it does not perform SMTP or TLS for lettre. Keep SMTP's
   local fake (it is protocol-specific); no reason to move it into testkit.
2. Override `send_outcome`. Before submission, proven refusal can be NotSubmitted.
   A lost final DATA response is Uncertain. Final 250 is Accepted; retain receipt
   when recording fails by filling recording_error. Never invent a provider queue
   id or treat 250 as recipient delivery. Preserve current conservative legacy
   `send` behavior; do not call both methods for one attempt.
3. Use typed DocumentSendAudit for accepted document metadata (provider id None is
   valid), retaining scope/actor/time. Switch closed error mapping where genuinely
   known to TlsRequired/CertificateRejected/OutboundTooLarge; do not infer a
   certificate cause from a generic network failure. Generic Timeout no longer
   instructs blind retry. Durable attempts/Message-ID ownership stay wave 4.

## Gmail

1. Implement `send_outcome` at the actual provider acceptance point in `src/lib.rs`.
   Retain the validated Gmail id/timestamp in SendReceipt even if bridge or audit
   recording subsequently fails; return Accepted with recording_error. Transport
   loss after POST is Uncertain, not a definite not-sent claim. No retry loop.
2. Replace the generic send-only audit call with `record_document_send_audit` and
   DocumentSendAudit. Keep the exact typed receipt, document ref, scope and actor.
   MemoryAuditSink exposes `document_sends`; FakeCoreBridge exposes `sent_receipts`,
   so remove the test wrapper that exists only to recover discarded receipts.
   The new sink default fails Audit: host must implement it before adoption.
3. Do NOT simply add a second history event around the existing
   `cx.core().record_document_send`. Existing Gmail is provider-owned history;
   retain that until wave 4 selects a single atomic history/audit owner. Rich audit
   is metadata support, not durability. When host owns the transaction, remove
   that plugin bridge call and make the host finalize exactly once using the
   returned receipt. The old generic send success audit should not also duplicate
   the rich send audit.
4. Adapt concrete begin/complete/disconnect to ConnectionLifecycle/PendingConnection;
   retain mailbox binding and provider-specific attempt state. Use the context's
   cancellation token in the framework wait. Return Disconnected with Confirmed
   or Unknown revocation instead of the local boolean-only DisconnectReceipt;
   NeedsSignIn/Expired/Revoked reflect evidence, not guesses. Host persists status.
   Keep local epochs/mutex until host lease/config-generation coordination passes
   race tests; these changes alone do not justify deleting serialization.

## Microsoft 365

1. Override `send_outcome` so Graph 202 returns Accepted with no invented id even
   when the local audit fails. Keep recording_error independently. Post-submission
   timeout/lost response is Uncertain. Do not claim delivered or automatically retry.
2. Adopt DocumentSendAudit for document ref and receipt (provider_message_id None),
   with a host sink that supports it. Retain existing host-only document history
   ownership unless wave 4 explicitly changes the contract; no duplicate recording.
3. Adapt start/complete/disconnect to ConnectionLifecycle/PendingConnection. Return
   Disconnected plus Unknown revocation on local-only disconnect (not Confirmed).
   Use `wait_cancellable` and scoped host OperationLease. Retain generation checks
   and the shared provider mutex until actual host-wide serialization exists.
4. Keep the numeric dynamic-port registration gate open. Tests show exact numeric
   bind/path/redirect consistency only; no real Microsoft registration or sign-in
   was attempted. Wave 4 must verify the intended native/public app configuration
   for personal and organizational accounts, with Jody starting real sign-in.
   Do not introduce localhost/fixed-port fallback or weaken callback checks.

## Shared host migration / per-gap disposition

- Callback client id: implemented and tested, provider migration still required.
- Document/message audit: additive typed extension, fail-closed sink, test capture;
  durable atomic recording is wave 4.
- Resolver-preserving host/size/deadline policy: implemented and tested, including
  protocol-neutral SMTP resolution. No resolver reset or host-limit expansion.
- Partial-feed warnings: additive rich slot methods/envelope, conservative legacy
  defaults; core reconciliation and richer provider-specific artifacts remain core.
- TLS/SNI/rebinding: implemented test-only verified-TLS route and synthetic fake;
  production private-address checks remain unchanged and run before test routing.
- OAuth lifecycle/status: optional object-safe interface and scoped updates;
  persistent registry transitions and provider adapters remain wave 4/provider work.
- Mail uncertainty: additive outcome contract, safe legacy errors, no automatic
  resend. Durable pre-submission attempt/outbox/recovery belongs in core wave 4.
- Cancellation/unlocked lifecycle: cooperative scoped handles and OAuth/HTTP/store
  checks implemented. Real unlock lease, commit serialization, crash handling and
  cross-instance config invalidation are core wave 4, not an AtomicBool guarantee.
- Dynamic-port registration: documented contract and local test; actual provider
  acceptance is explicitly unverified. No provider registration change made.

Other requests: fixed usage/subscription/setup-token/TLS/outbound/uncertain error
codes are included. Live SSE streaming, failed-read byte accounting, alternate-auth
manifest design, install settings, receipt consent/artifacts, and durable state are
not silently designed in this framework fix. No plugin workarounds were removed in
these branches: follow-up provider changes and host verification are required.
