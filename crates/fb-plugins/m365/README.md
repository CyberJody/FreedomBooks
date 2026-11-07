# Microsoft mail document sender

`fb-plugin-m365` fills the `DocumentSender` slot using delegated Microsoft Graph
`POST https://graph.microsoft.com/v1.0/me/sendMail`. It sends the core-rendered
PDF as `document.pdf`, with any additional supplied file attachments, plain-text
body, subject and explicit recipients. It saves a copy to Sent Items. It neither
renders documents nor writes to the books. The host records the returned receipt.

This is the outside-system half only. There is no Electron/settings UI or live
credential persistence here, and no real account was used to develop/test it.
The Python reference has no Microsoft sender to translate; this is a new plugin.

## Authentication and permissions

- OAuth2 public client, authorization code with PKCE S256, framework loopback
  listener on `127.0.0.1:<ephemeral-port>/callback`, ten-minute timeout, one-shot
  callback and state validation. Open its secret authorization URL only in the
  system browser; do not log it.
- Fixed Microsoft `common` v2 authorization/token endpoints support the intended
  personal and organizational account audience. Select-account is requested so
  choosing the mailbox is explicit. No national-cloud endpoints in this version.
- Requested scopes are exactly `Mail.Send offline_access`. No `User.Read`,
  `Mail.Read`, `Mail.ReadWrite`, application permission, client secret or password.
- This is OAuth authorization, not an OIDC identity/login feature. No `openid`,
  profile or email claim is requested or trusted. The framework generates a nonce,
  but there is no ID-token nonce claim to verify in this flow. Any non-null
  unsolicited `id_token` is rejected, rather than decoded and trusted. Adding OIDC
  later requires signature/issuer/audience/expiry verification followed by the
  framework's `grant.verify_nonce`; a callback nonce is never an identity proof.
- The public application/client UUID is supplied to `M365Sender::new`; no borrowed
  third-party registration or placeholder registration is compiled in. Forks need
  their own registration.
- Tokens live only in the context's scoped `CredentialStore`, with `SecretString`
  in memory and zeroizing serialization buffers. Production at-rest encryption
  belongs to the host. Testkit's memory store is test-only.
- Refresh starts within 60 seconds of expiry. Save rotated credentials before
  submitting mail; retain the old refresh token if Microsoft omits a replacement.
  Invalid/revoked grants clear credentials and require sign-in; temporary and
  malformed responses retain existing credentials. A Graph 401 clears credentials
  and asks for sign-in without replaying the send. A 403 reports permission refusal.
- Disconnect forgets local credentials only. It does NOT claim server revocation,
  clear browser cookies, or sign out all Microsoft sessions. The user/admin must
  remove this application's consent in Microsoft's account/tenant controls if
  upstream access must be revoked. Do not request extra Graph permissions for a
  broad `revokeSignInSessions` operation.

## What leaves the machine

The manifest in `src/lib.rs` is the consent inventory. The plugin reads only the
outgoing document and recipients handed to it by the core; it never browses the
mailbox or the book's tables.

Microsoft receives the recipient addresses/display names, subject, plain-text
body, rendered PDF bytes, extra attachment names/types/bytes, the public client
ID, and OAuth protocol material (authorization code, PKCE verifier, access/refresh
tokens as appropriate). Microsoft delivers the message and attachments to the
chosen recipients and keeps a Sent Items copy. No FreedomBooks company/document
UUID is included in the Graph payload. No provider response, email body, address,
token or authorization URL enters logs, errors or audit metadata.

HTTP goes only through `PluginContext::http()`, with the manifest's exact hosts
`login.microsoftonline.com` and `graph.microsoft.com`. The framework enforces
HTTPS, DNS/public-address validation and pinning, no redirects/proxy, and size/time
limits. There is no production endpoint-override API or insecure feature in this
crate. Test endpoint overrides exist only in unit-test compilation.

## Delivery semantics and local limits

- Use `send_outcome`: Graph 202 returns Accepted with a no-id receipt, retained
  even when rich audit fails (independent recording_error). A lost final response
  is Uncertain; proven preflight/provider refusal is NotSubmitted. No invented
  message id or recipient-delivery confirmation. Legacy send remains conservative
  and returns errors for recording/uncertain failures, not proof of non-delivery.
- No automatic send retry, including timeout, 429, 5xx, authentication failure or
  audit failure. A request can have reached Microsoft before an error became
  visible. Check Sent Items before explicitly trying again. These outcomes need
  careful UI wording in wave 4; see `FRAMEWORK-REQUESTS.md`.
- Conservative simple-send limits: 2,800,000 aggregate raw PDF/attachment bytes,
  3,900,000 serialized JSON bytes, 64 KiB plain-text body, 100 recipients, 20 extra
  attachments, 998-byte subject, 255-byte extra filenames. All attachments use
  `#microsoft.graph.fileAttachment` with standard base64. No upload sessions, large
  attachment API, chunking, sender impersonation, shared-mailbox permissions,
  CC/BCC or HTML are implemented. These are local product limits, not claims about
  every Microsoft mailbox's configured maximums.
- Basic recipient/control-character validation precedes all credential/network
  work. Filenames cannot be paths. JSON escaping is performed by serde, not string
  concatenation. Invalid or oversized documents never trigger a token refresh.
- Token responses have an additional 64 KiB parsing limit; the host should set its
  HTTP response cap to 64 KiB too, so the transport rejects large bodies before
  buffering them. A request cap of `MAX_MESSAGE_BYTES` and a finite timeout (the
  framework default is 30 seconds) are appropriate for this plugin.

## Host wiring (wave 4)

1. Construct one `M365Sender` per configured client ID, register its static
   manifest and `Arc<dyn DocumentSender>` in `PluginRegistration`, and obtain an
   enabled, unlocked, scoped context from the host. Registration is not consent.
2. Call `start_authorization`, hand only `authorization_url()` to the trusted
   system-browser opener, then run `complete_authorization` off the UI thread.
   Dropping `PendingAuthorization` closes its listener. The pending object is bound
   to company, connection and client ID. A local attempt generation prevents an
   older callback from saving credentials after a newer Connect or disconnect on
   the same sender instance. The host must still drop cancelled listener objects
   promptly, cancel jobs on book lock/close or configuration change, and recheck
   the unlock lease before completion. Cross-instance/process lifecycle is not
   coordinated by this crate. Optional `connection_lifecycle` adapts these methods
   using secret JSON `{}` for begin; completion returns scoped Connected/expiry and
   disconnect returns Disconnected with Unknown upstream revocation. The context's
   host-issued OperationLease cancels the framework wait and blocks exchange/save.
   Keep provider generations/mutexes until real host-wide leases are integrated.
3. On success mark the connection connected; on `NeedsSignIn`, mark it accordingly.
   The host supplies encrypted credentials, consent, book-lock enforcement and
   lifecycle serialization. Keep the same sender instance: its mutex serializes
   refresh/send/disconnect so concurrent sends rotate once. This deliberately also
   serializes different connections sharing that instance; no cross-process
   coordination or multiple independently constructed sender instances is claimed.
4. Run `send_outcome` off the UI thread and record the returned receipt through the host's
   normal `CoreBridge::record_document_send` path. Do not post accounting entries.
5. Call `disconnect`, then update connection state and local-forget/revocation
   metadata without claiming Microsoft revocation.

Connect completion, disconnect and every send success/failure attempt emit a
closed-code audit event. Accepted sends use DocumentSendAudit with exact receipt,
document ref and scope/actor/time, not a duplicate generic success audit. Host-only
document history ownership is unchanged; the host must implement the typed sink.
A pending attempt that is dropped before completion has
no plugin-completion event; the host owns UI cancellation bookkeeping. Byte counts
are attempted request-body bytes and received response-body bytes (not proof of
wire delivery and not headers); refresh traffic is included in the send event.

## Jody's later Entra setup: evidence and open checks

No external network calls or real Microsoft sign-in were allowed in this job.
Consequently, fake-test verification below is NOT verification of current Entra
portal policy. The following registration/consent steps are marked honestly:

- **Verified locally:** code uses `common`, only `Mail.Send offline_access`, public
  client PKCE, and no client secret/User.Read; fake token/Graph contracts pass.
- **Likely (Microsoft documented model; not re-fetched here):** Jody needs access
  to a Microsoft Entra tenant and permission to register applications. Register a
  new app with supported accounts "Accounts in any organizational directory and
  personal Microsoft accounts". Copy its application/client ID into this plugin's
  host configuration. Configure it as a native/mobile-and-desktop public client,
  not a confidential web application; do not create or ship a client secret.
- **Likely:** add delegated Microsoft Graph `Mail.Send`, remove automatically added
  `User.Read` if unused, and request `offline_access` at authorization for refresh.
  A user with a personal Outlook.com mailbox or a work/school Exchange Online
  mailbox consents to sending on their behalf. A Microsoft account alone does not
  guarantee that it has a usable mailbox.
- **Unknown / explicit wave-4 gate:** confirm the exact current Entra manifest or
  portal procedure for registering `http://127.0.0.1/callback` for this public-client
  account audience, and whether its redirect matching accepts the helper's dynamic
  port. Some Microsoft guidance/UI examples use `http://localhost` and differences
  for numeric loopback/portal entry matter. Do not silently switch to localhost,
  a fixed wildcard listener, device flow, or a confidential-client secret to make
  a test pass. If numeric dynamic-port loopback is not accepted, coordinate a
  reviewed framework adaptation before real sign-in; this fake suite cannot prove
  the provider's redirect-registration behavior.
- **Likely:** delegated `Mail.Send` normally permits user consent, but a work
  tenant's user-consent, publisher-trust or conditional-access rules may require
  tenant-admin consent or block it. Entra "Allow public client flows" guidance
  should be checked for the registered native platform; do not enable unrelated
  password/device flows just by assumption. **Unknown:** Jody's and each customer's
  actual tenant policies, eligible accounts and admin access.
- **Likely:** configure app branding, publisher domain, homepage/privacy/terms as
  required for consent. For a verified-publisher badge, complete Microsoft's
  current Partner Center/Microsoft AI Cloud Partner Program organization
  verification, associate the eligible partner identity with the Entra app, and
  meet Microsoft's role/domain/MFA prerequisites. Publisher verification is not
  mail permission or a guarantee of tenant consent. Unverified apps may be usable
  for development but blocked by customer consent policies; do not promise public
  availability without it. **Unknown:** current exact prerequisites and Jody's
  eligibility/verification status. No registration, consent, domain verification,
  Partner Center operation or publication was performed here.

Documentation entry points for Jody/lead to verify before wave 4 (not fetched in
this fake-only job):

- https://learn.microsoft.com/en-us/graph/api/user-sendmail?view=graph-rest-1.0
- https://learn.microsoft.com/en-us/graph/permissions-reference#mail-send
- https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-auth-code-flow
- https://learn.microsoft.com/en-us/entra/identity-platform/reply-url
- https://learn.microsoft.com/en-us/entra/identity-platform/quickstart-register-app
- https://learn.microsoft.com/en-us/entra/identity-platform/publisher-verification-overview

## Testing

From the v2 worktree root:

```text
cargo test --offline -p fb-plugin-m365
cargo clippy --offline -p fb-plugin-m365 --all-targets -- -D warnings
cargo fmt -p fb-plugin-m365 -- --check
cargo test --offline -p fb-plugin-api -p fb-plugin-testkit
```

Tests use `FakeOAuthProvider`, `FakeServer`/`ScriptedServer`, `FakeCoreBridge`,
`MemoryCredentialStore`, `MemoryAuditSink` and `FixedClock`. Only numeric loopback
HTTP and synthetic data are used. Dev-dependency feature unification enables the
framework's test seam; never link the testkit into production. No real account,
mail delivery, mailbox, tenant registration, provider nonce/identity assertion or
Internet request is tested. Tests cover PKCE exchange, callback state/denial,
scoping, refresh rotation/concurrency/revocation, no resend, HTTP policy/limits,
transport and credential/audit failures, payload encoding and validation. No
preview/screenshots apply to this library-only deliverable.
