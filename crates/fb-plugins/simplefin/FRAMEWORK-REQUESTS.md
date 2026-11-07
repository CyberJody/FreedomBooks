# Framework requests — W1b SimpleFIN

Updated for W1-FIX1 (`66859ce`). This follow-up changes only the SimpleFIN crate;
no framework, testkit, workspace manifest or sibling plugin is edited.

## Resolved and adopted

1. **Constrained transport / URL preflight.** `src/client.rs::execute` now calls
   `cx.http().constrained(timeout, 1, cap)?.execute(request)`. It retains the host
   resolver, allow-list, cancellation and TLS test route. Each limit is the minimum
   of the host ceiling and the protocol ceiling (claim 4,096 bytes / 30s; accounts
   20 MiB / 90s). The reconstructed transport and empty-DNS/Network-sentinel URL
   adapter are removed; preflight uses `HttpPolicy::validate_url` directly.
   Plugin-path regressions enforce the reviewer's 16-byte host cap and 25ms host
   deadline against a 125ms response, for claim and accounts. Claim and account
   cap tests cover Content-Length and unframed streamed bodies. Resolver tests
   exercise the actual plugin and verify the injected deadline budget.

2. **Rich BankFeed results.** SimpleFIN overrides `list_accounts_result` and
   `fetch_result`; each performs one provider request and one parse, returns valid
   items even on partial feeds, and carries closed warning codes, normalized IDs,
   source indexes, inventory and account metadata. Inventory includes identifiable
   accounts skipped for bad currency/balance; explicit `accounts: []` is known-empty.
   Missing/non-list accounts is InvalidResponse, never invented empty inventory.
   `parse::AccountSet` and `fetch_account_set` remain intentional protocol APIs for
   full transaction epochs, skip reasons and sanitized diagnostics, not workarounds.
   Legacy Vec methods (and `fetch_and_stage`) still refuse partial results.

3. **Safe provider error codes.** Claim HTTP 403 maps to SetupTokenRefused;
   established-feed HTTP 403 stays NeedsSignIn. HTTP 402 maps to
   ProviderSubscriptionRequired. Responses are never copied into user-safe errors.

4. **Executable TLS/SNI/pinning proof.** Plugin-path tests now use FakeTlsServer
   and the test-only exact validated-address route. They establish real verified
   TLS to the synthetic peer and assert original Host/SNI, one injected resolution
   per request, wrong-hostname/untrusted-certificate rejection, and refusal of
   later private or mixed public/private DNS answers before another connection.
   The framework TLS fake serves the fixed body `ok`: the test proves completed
   transport via peer observations and two received bytes, then expects the plugin
   to reject that non-JSON body. It does not claim a successful TLS Account Set sync.
   Successful protocol exchanges are covered separately by numeric-loopback fakes.
   No production TLS bypass or provider-owned socket/resolver was added.

## Remaining framework gap

**Failed-transfer byte accounting:** errors still lack consumed-byte metadata.
Audit records completed response body bytes and zero request body bytes (SimpleFIN
sends none). An oversized or timed-out read cannot accurately report bytes consumed;
this plugin does not fabricate them. This is not a host-policy bypass.

## Host obligations already assigned to wave 4 (not new plugin scope)

- Persist credentials with the encrypted scoped CredentialStore; the testkit store
  is deliberately in-memory, not evidence of encryption at rest.
- Own real unlocked-book leases, per-connection connect/sync/forget serialization,
  consent, polling interval/overlap windows, dedupe and transactionally coordinated
  audit/credential changes. A failed save after claim may consume the upstream
  token; never blindly retry after timeout or persistence failure.
- Integrate through the rich slot, preserving completeness and warnings. Check
  BOTH `has_complete_inventory` and the intended account/date/pending query scope
  before withdrawing absent pending rows; balances-only is not a transaction pull.
- Pass the book timezone, not the machine timezone. DateWindow is inclusive local
  calendar days, including daylight-saving changes.
- Stage exactly once: rich `BankFeed::fetch_result` plus host review/staging, OR the
  legacy complete-only `fetch_and_stage` convenience method. Never both. Matching,
  pending withdrawal and posting remain host/core responsibilities.
