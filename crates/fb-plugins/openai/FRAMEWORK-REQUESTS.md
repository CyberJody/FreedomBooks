# Framework requests for OpenAI (W1b)

No framework files are changed by this crate. These are integration gates, not
claims that wave 4 is already wired.

## Closed in W1b-ADOPT (framework 66859ce)

Callback client id now comes exclusively from `OAuthGrant.returned_client_id`
after the state-validated cancellable wait. The out-of-band argument is removed.
Dynamic shape, reauthorization identity, signed audience and stale-generation
checks remain; fake redirects exercise the real callback parameter.

Request classes use resolver-preserving `constrained`: metadata 30 s / 2 MiB,
completed SSE 180 s / 16 MiB, intersected with host ceilings. 429 maps to the
closed UsageLimit code without clearing credentials. Live streaming is deferred.

Object-safe ConnectionLifecycle wraps existing OAuth/disconnect operations;
Connected/Disconnected updates include scoped expiry and explicit revocation
status. Cancellation and generation/mutex checks remain; host persistence and
atomic unlock/save coordination remain wave 4.

## Install-wide host identity

CoreBridge state is per connection. `ext_agent_host_id` must be one stable `urn:uuid:` identity per
install, including multiple companies and reconnect after Forget. Host should
create/persist it in its install settings before sign-in and pass that UUID to
`OpenAi::new`. The plugin includes it on every authorization and persists it in
its encrypted credential record. Generating a fresh UUID on each constructor call
would be wrong. Do not put tokens into connection state to simulate global settings.

## Receipt review metadata and authoritative Save checks

DraftBill has vendor/date/lines only; ImportedLine has no printed amount, source
index, receipt reference number, due date, subtotal/total, or warning/override
metadata. AiRequest also has no consent recheck capability.

Local workaround: `read_receipt` requires a host `ReceiptConsent` checked before
and after token refresh; it returns a ReceiptReading preserving all header fields,
printed amounts, products and mismatch marks. `stage_receipt_draft` calls only
CoreBridge::create_draft_bill and REFUSES mathematically mismatched lines. It does
not fabricate a repair, discard the original amount, post, or implement the user's
"Keep as entered" choice. A low-level AiHelper caller must enforce the host consent
lease itself; production receipt entry points must use the checked wrapper.

Requested for wave 4: a core-owned receipt reading artifact, associated to the
draft in the same transaction, plus indexed authoritative printed amounts,
review/override audit and complete header metadata. The host must preserve the
returned ReceiptReading before presenting the editable bill. No workaround writes
receipt contents into a per-connection cursor (that would overwrite readings and
lose the bill link). Vendor/account matching and Save/Keep-as-entered UI remain
core responsibilities, as in the design. The provider crate should not own them.

## Manifest/account state gaps

AuthKind describes only one auth mode; the manifest declares OAuth and advertises
its secret API-key fallback field. DataKind cannot label OIDC identity/registration
metadata; scopes plus README describe email/profile, install UUID and tokens.
ConnectionStatus and revoke_confirmed persistence are host responsibilities;
the lifecycle adapter returns typed revocation/status while `NeedsSignIn` is surfaced by the slot.


## FRAMEWORK-4: zeroize retained authorization hints (not fixable plugin-locally)

The current `OAuthConfig::extra_authorization_parameters` is a
`Vec<(String, String)>`. `OAuthLoopback::start(config)` consumes that config and
retains it in its private `config` field after building the secret authorization
URL (`fb-plugin-api/src/oauth.rs:109-116, 136-143, 203-218` at W1 base 5c0bfff).
There is no public mutable accessor or URL-built hook. Consequently the OpenAI
plugin cannot zeroize `id_token_hint` (or the private account `login_hint`) once
that URL has been constructed; cloning config would only add another plain copy.

Requested: the helper must zeroize and discard extra parameter values immediately
after URL construction, including cleanup on construction errors, or accept
secret-owning values and move them through to zeroizing ownership. A future
framework regression must prove the pending listener no longer retains plain
hint values after URL creation, while the secret URL still contains the hints.
This is a security/integration gate, not a claim that plugin-only FIX1 fixed it.

No framework files are edited here. W1-FIX1 provider contracts are adopted;
FRAMEWORK-4 remains a separate W1-FIX2 security gate. The existing login/ID-token hints are kept to preserve the frozen
Python reauthorization behavior, not silently omitted as a workaround.
