# Gmail document sender

`fb-plugin-gmail` implements the framework's `DocumentSender` slot. It sends a
core-rendered PDF, a plain-text message and optional attachments through Gmail's
`users.messages.send` API. It cannot read a mailbox, render a document, access a
database, or post accounting entries. This is a new adapter; the frozen Python
reference has no Gmail/document-email implementation to translate.

## Consent and outbound data

The static `MANIFEST` is the consent inventory:

- Account: Gmail or Google Workspace mailbox.
- Only OAuth scope: `https://www.googleapis.com/auth/gmail.send`.
- Reads from the host: the selected document, PDF, subject, body, recipients,
  extra attachments and the mailbox selected for this connection.
- Sends to Google: that complete email (including attachment names and bytes),
  sender and recipient addresses. Google delivers it to the recipients and may
  retain it in the sender's Sent mailbox. No bank records, unrelated documents,
  ledger data or incoming mail are requested.
- Authentication sends the public desktop client id, authorization code, PKCE
  proof, loopback URI and, if supplied by the desktop registration, its client
  secret to Google's token endpoint. Refresh and revocation send tokens only to
  Google's token/revocation endpoints; revocation uses a form body, not a URL.
- Exact hosts: `accounts.google.com`, `oauth2.googleapis.com`,
  `gmail.googleapis.com`. All API calls use `PluginContext::http()`; production
  permits HTTPS only, no redirects/proxies, public-address DNS pinning and the
  framework's byte/time caps. Endpoint overrides exist only inside unit tests.

Never log the authorization URL, token responses, email payloads or underlying
transport errors. Errors are the framework's fixed `PluginError` variants. Audit
carries closed codes, scoped ids, attempted request-body/received-body byte
counts, duration and clock time, not addresses or contents. A network failure
cannot establish how many attempted bytes actually reached Google.

## Host integration (wave 4, not implemented here)

1. Show the manifest consent page and collect the connection's mailbox address.
   The user must use the authenticated Gmail address or an already authorized
   send-as alias. No extra read scope is requested to discover the mailbox or
   enumerate aliases; Google enforces send-as permissions.
2. Register one shared `Arc<GmailSender>` made with
   `GmailSender::new(client_id, optional_secret)`. Supply the optional secret as a
   `SecretString`, never a logged settings value. The installed-app client id
   (and any bundled desktop client secret) is not a confidential web-app secret.
   There is deliberately no real client id, project or account in this crate.
3. While the book is unlocked and the connection enabled, call
   `begin_authorization(cx, from_address)`. This returns a one-shot
   `GmailAuthorization`. Open `authorization_url()` in the system browser, not an
   embedded browser, then call `complete_authorization(cx, attempt)` off the UI
   thread. Dropping the attempt closes its socket.
4. The framework binds `127.0.0.1:0`, uses PKCE S256, validates state, exact callback
   path and authority, and times out after ten minutes. The plugin requests
   `access_type=offline` and `prompt=consent` so reconnect can obtain a refresh
   token. Attempts are company/connection/client-bound; a second attempt or
   disconnect invalidates a pending attempt. No OpenID scopes are requested and
   no ID-token identity is trusted. The helper generates a nonce, but there is
   no issued OIDC identity/nonce claim to authenticate with this send-only flow.
5. Token data and the chosen From address live only in the scoped credential
   record. The host must provide encrypted `CredentialStore` persistence, recheck
   its unlock lease and maintain Connected/NeedsSignIn/Disconnected status. The
   testkit memory store is not encrypted persistence. Different connections can
   use different mailboxes through the same shared sender and store.
6. Pass the core-rendered `OutgoingDocument` to `send_outcome`. MIME is built using
   lettre's builder only (no SMTP/socket/TLS transport features), with base64
   parts and base64url Gmail JSON. The caller's clock sets Date and sent time.
   Controls/header injection, malformed addresses, invalid filenames/media types
   and empty PDFs/recipients are refused. Limits are 100 recipients, 20 extra
   attachments, 8 MiB combined unencoded content and 16 MiB formatted MIME; the
   HTTP policy may impose a lower cap. ASCII addresses are required; Unicode
   subjects, display names, body text and attachment filenames are supported.
7. After acceptance the plugin records document history once through CoreBridge
   and rich DocumentSendAudit metadata with the validated message id, exact receipt,
   document, scope, actor and timestamp. No second generic success audit is emitted.
   Accepted retains its receipt and independent recording_error even if either local
   step fails. The host must implement the typed sink and must not duplicate the
   existing provider-owned history event. Atomic durability remains wave 4.
8. `disconnect` attempts upstream revocation with the refresh token and always
   attempts local forget, including after an upstream failure. The returned
   `DisconnectReceipt::revocation_confirmed` is true only for a Google 200.
   Local storage or audit failures are errors, not successful forget claims.
   If revocation is unconfirmed, advise the user to remove access from their
   Google account as well. Staged/bookkeeping data is never touched.

Refresh runs sixty seconds early, serializes with send/forget on the shared
sender, saves a rotated token before sending and retains the old refresh token
when Google omits a replacement. `invalid_grant` clears unusable credentials and
returns `NeedsSignIn`. A send 401 also clears credentials and requires sign-in;
it does not silently retry. Temporary provider errors retain credentials.
No message send is automatically retried, including on 429/5xx/timeouts.

Use the rich outcome: known pre-send/provider refusal is NotSubmitted; a lost or
malformed final response after POST is Uncertain. Known acceptance survives local
recording errors. Never automatically resend any outcome; the user checks Sent.
Legacy send still returns errors for uncertain/recording failures and must not be
interpreted as proof of non-delivery. Google acceptance is not recipient delivery.

The optional lifecycle adapter accepts secret JSON `{"from_address":"..."}` and
returns scoped Connected/Disconnected status, expiry and Confirmed/Unknown
revocation. It preserves epochs/mutexes and uses cancellable callback waiting.
Host persists registry status and coordinates real unlocked-book leases in wave 4.

Secret buffers and owned tokens are zeroized and never implement unredacted
Debug. JSON token fields deserialize directly into secret owners. lettre and
HTTP/JSON libraries can allocate transient plaintext copies; this is not a claim
that every byte of process memory is wiped or that a locked-book lease can stop
an already-started network request. The host owns that lifecycle boundary.

## Real Google sign-in checklist for Jody (later, on his go)

No Google Cloud project, consent screen, account connection, live mail or real
network/API call was created or used in this job. Policy information below is
**likely**, not freshly verified: this brief permits fake-server network work
only. The linked Google documentation must be checked before registration and
publication. Google's policies and console labels can change.

1. In a Google Cloud project owned by Jody, enable the Gmail API. Set up Google
   Auth Platform/OAuth consent branding (app name, support/developer contacts),
   audience, requested data access and named test users. Use External for public
   users, not an Internal-only Workspace registration.
2. Create an OAuth client of type **Desktop app**, not Web application. Supply its
   client id and, if issued/required for its desktop token exchange, client secret
   to the host's protected configuration. Use the system browser and numeric
   loopback redirect; no remote callback web server is needed. Forks should
   register their own project/client rather than impersonating this registration.
3. Request only `gmail.send`. Scope verification class: **likely Sensitive, not
   Restricted**. Therefore plan for sensitive-scope OAuth app verification for a
   broadly distributed External app; do not assume a restricted-scope security
   assessment is needed solely for this scope. The current Google classification
   has not been fetched/verified in this job.
4. During Testing, explicitly add Jody's chosen account as a test user. Expect an
   unverified/testing warning and testing audience limits (likely up to 100 test
   users). External Testing refresh tokens with this Gmail scope likely expire
   after seven days. Testing is not a public release plan.
5. Before public distribution, prepare a public homepage, privacy policy,
   verified authorized domains, accurate branding/contact details, a scope-use
   justification and a demonstration video showing consent and document sending.
   Submit the sensitive-scope verification and comply with the Google API
   Services User Data Policy/Limited Use requirements. Exact exemptions,
   requirements and review times remain unverified; do not promise a deadline.
6. Workspace administrators may additionally block or require approval for the
   app even after Google verification. Jody conducts the actual sign-in, alias
   permission check, refresh and revocation test in wave 4, with test documents
   and his explicit approval. No worker performs them automatically.

Documentation to verify later (reference URLs, not fetched during this job):

- https://developers.google.com/workspace/gmail/api/auth/scopes
- https://developers.google.com/identity/protocols/oauth2/native-app
- https://developers.google.com/identity/protocols/oauth2#expiration
- https://developers.google.com/identity/protocols/oauth2/web-server#tokenrevoke
- https://developers.google.com/workspace/gmail/api/guides/sending
- https://developers.google.com/identity/protocols/oauth2/production-readiness/sensitive-scope-verification
- https://developers.google.com/terms/api-services-user-data-policy

## Offline tests

From the workspace root:

```sh
cargo test --offline -p fb-plugin-gmail
cargo clippy --offline -p fb-plugin-gmail --all-targets -- -D warnings
cargo test --offline -p fb-plugin-api -p fb-plugin-testkit
```

Tests use only synthetic testkit `FakeServer`, `ScriptedServer`,
`FakeOAuthProvider`, `FakeCoreBridge`, `MemoryCredentialStore`, `MemoryAuditSink`
and `FixedClock`. The testkit validates the actual authorization code, PKCE,
client id and redirect; a local fake translates its deliberately minimal token
response into a Google-shaped response. No fake ID token is trusted as identity.
Other tests cover MIME/binary recovery, ids at the document-send boundary,
refresh/rotation/concurrency, revocation/failure cleanup, stale/wrong-scope OAuth,
provider denial, malformed/oversized input, HTTP guards and credential/audit/core
failure injection. Production never links the testkit or enables test support.

An optional independent MIME parser check requires Python 3 (standard library
only). Set `FB_GMAIL_MIME_PROBE` to a synthetic artifact path under the job scratch
folder, run the `mime_has_recoverable_binary_attachments_and_an_injected_date`
test, then run `python3 crates/fb-plugins/gmail/tests/verify_mime.py <that-path>`.
The ordinary cargo suite does not require Python or write a fixture file.
