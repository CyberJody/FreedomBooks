# SMTP document sender

`fb-plugin-smtp` is the first-party `DocumentSender` plugin for FreedomBooks v2.
It sends a core-rendered PDF, a plain-text email body, recipients, subject, and
optional attachments using a user's SMTP account. It does not render PDFs,
read mail, access a database, or post accounting entries. This is a new plugin;
there was no Python document-email implementation to translate.

The mail library is pinned to **lettre =0.11.23**, confirmed on crates.io on 2026-10-30:
<https://crates.io/crates/lettre>. It uses Rustls, Ring and WebPKI roots rather
than a system OpenSSL dependency. The production dependency on `fb-plugin-api`
has default features disabled: SMTP does not need an HTTP client.

`src/transport/connection.rs` owns the bounded reply-reading loop and submission
sequence, using lettre's MIME, command/AUTH, response parsing and TCP/Rustls
helpers. It does **not** use lettre's `AsyncSmtpConnection::read_response`: in
0.11.23 that method checks size only after an unbounded `read_line` finishes.
A wrapper below TLS would not protect decrypted replies. The lower-level
`AsyncNetworkStream` API is deprecated upstream, isolated to this module and
version-pinned; re-review it and rerun the full transport tests before upgrading.

## Consent, settings and scopes

The executable manifest is `MANIFEST` in `src/lib.rs`. It declares:

- Slot: `DocumentSender`; authentication: `Password`.
- Scope: `smtp.send` (a descriptive plugin permission, **not** an OAuth scope).
- Reads/sends: document contents and recipient addresses.
- Settings: host, port, encryption mode, sender email, secret username and
  secret app password.
- Static host list: empty. This is intentional, not a wildcard. The host must
  show the selected SMTP hostname and obtain consent, then include that exact
  hostname in the context's `HttpPolicy`. Without it, sending fails closed
  before DNS resolution. Changing provider host requires new host consent.

Use `SmtpConfig::new(host, port, Security::StartTls | Security::ImplicitTls,
from)` and then `SmtpSender::new(config)`. Typical submission ports are 587
(STARTTLS) and 465 (implicit TLS). Port 25, port zero, URL/userinfo/path injection
and malformed hostnames are rejected. Hostnames are ASCII DNS names (use their
punycode spelling for international names); IPv4 literals are also accepted but
must pass the public-address guard. IPv6 DNS answers are supported; literal
IPv6 configuration is not currently supported.

No plaintext or opportunistic TLS mode exists. STARTTLS must be advertised and
complete successfully before AUTH. Both modes verify the certificate chain and
original SMTP hostname, require TLS 1.2 or later, and authenticate only after
checking that the connection is encrypted. PLAIN and LOGIN are supported **only
inside TLS**. There are no certificate-verification bypasses in production.

The username and app password are stored only through the context's scoped
`CredentialStore`, using `SmtpSender::save_credentials`. Both inputs are
`SecretString`; the versioned binary record is `SecretBytes`. This helper saves
settings only: it does not connect, test credentials, or mark the connection
connected. The host owns consent, connection status, connect/disconnect audit,
encrypted storage and forgetting. There is no provider-neutral SMTP revocation
protocol: users revoke an app password at their mail provider.

## Data that leaves the machine

The configured mail provider receives:

- SMTP username and app password inside the authenticated TLS connection;
- sender and every selected recipient address (all recipients are `To`, not Bcc);
- subject, plain-text body, the PDF under the neutral filename `document.pdf`;
- names, media types and bytes of explicitly supplied additional attachments;
- normal email metadata, including the host-clock Date and generated Message-ID.

The server hostname goes to the host-supplied framework DNS resolver. EHLO uses the fixed
name `freedombooks.invalid`, not the user's computer name. There are no HTTP
calls, proxies, provider redirects, or unrelated network endpoints. SMTP TLS
protects submission to the provider; it is **not end-to-end email encryption**.
A successful SMTP submission is not proof of delivery to the recipient's inbox.

Neither audit nor returned errors contain credentials, recipients, email bodies,
subjects, PDFs, raw SMTP errors, or arbitrary provider response text. The receipt
has `provider_message_id: None`: SMTP has no standard queue-id field safe to
assume. The send event is `plugin.smtp.send`, with closed outcome/purpose codes,
scope/actor/time, duration and counts. The success byte count is serialized MIME
bytes, not TLS/SMTP wire bytes. On failure it is zero/unknown; this does **not**
prove that no bytes were transmitted. No provider response text is counted or
retained.

Our owned credential and serialized-message buffers use secrecy/zeroization.
Lettre necessarily creates transient owned copies during MIME encoding and AUTH;
its internal strings are not guaranteed to be zeroized. Do not enable lettre
protocol tracing or dump transport/message objects in the host.

## Limits and transport hardening

- Default size cap: 10 MiB; configurable up to a hard maximum of 20 MiB. Both
  total raw inputs and the final encoded MIME message must fit; base64 overhead
  counts. This is a message-size limit, not a precise memory-use or wire-byte cap.
- At most 50 recipients and 10 additional attachments. Reject header control
  characters, malformed mailboxes/media types and path-like attachment names.
- PDF magic must begin `%PDF-`; the trusted core owns actual PDF rendering and
  validation. This prefix check does not claim full PDF validation.
- Default attempt timeout: 30 seconds; configurable to at most 120 seconds.
  DNS uses the context's pinned `resolve_endpoint`, preserving its injected
  resolver and stricter host timeout ceiling. Returned remaining time then bounds
  the complete async connect/TLS/EHLO/AUTH/DATA operation, including slow-drip
  responses. The future and socket are dropped on timeout; there is no detached
  mail-sending worker. The framework's timed-out system DNS thread can finish
  separately, but cannot send mail. Host credential/audit callbacks themselves
  are synchronous and must be bounded by the host.
- The context's exact host allow-list and public-address guard validate every
  DNS answer, refusing the entire set if any answer is private, local, reserved,
  mapped/transition, or the wrong port. Lettre receives only pinned numeric
  addresses; TLS still verifies the original hostname. No second DNS lookup.
- The plugin enforces **1,000 bytes per reply line** and **100,000 bytes per
  complete response**, including status prefixes and CRLF. Checks happen before
  copying each read fragment, so a missing newline cannot defer the ceiling
  until EOF or timeout. The same reader handles greeting, EHLO, STARTTLS, AUTH,
  MAIL/RCPT, DATA and final acceptance, before and after TLS. On overflow it
  returns fixed `InvalidResponse` and drops the socket without waiting for QUIT.
- Reply accumulation uses a fixed 100,000-byte boxed slice and a 1,024-byte input
  buffer. This bounds framing storage, not total process memory: TLS record
  buffering and lettre's bounded-response parsing have their own allocations.
  Each response resets the budget; continuation lines do not. Unexpected
  plaintext read-ahead at the STARTTLS boundary fails closed.
- Raw server errors are converted to fixed `PluginError` variants.
- AUTH must return 235; message acceptance must return 250 after DATA. Acceptance
  is the commit point; a stalled/missing QUIT cannot turn an accepted message into
  a failed send. No automatic retries, account deletion or password replacement.

## CoreBridge integration (wave 4)

The synchronous slot expects an `OutgoingDocument` from the host's scoped
`CoreBridge::rendered_document`. Run it off the UI thread. The host must verify
company access, enabled connection, unlocked-book lease and user send consent
before giving the plugin its context. Use `send_outcome`; after acceptance, the host calls
`CoreBridge::record_document_send(document_ref, receipt)` to update document
history. The plugin records its own typed DocumentSendAudit (document/receipt/scope/actor/time); it does not duplicate the host's
document-history write. Tests exercise this complete fake-bridge sequence.

SMTP cannot offer exactly-once delivery. Rich outcomes distinguish known
pre-submission refusal (NotSubmitted), final 250 (Accepted) and a lost response
or cancelled/failed I/O after DATA starts (Uncertain). Accepted retains its receipt
when rich audit fails, with an independent recording_error. No id is invented.
Legacy send still returns errors for uncertain/recording failures; that is not
proof mail was not sent. Never automatically resend. Durable attempts, recovery,
Message-ID ownership and atomic history/audit remain wave 4.

Cancellation is checked before connect/auth/DATA and every 20 ms during I/O;
cancelling drops the future/socket but cannot undo a provider submission. The host
must coordinate actual book unlock and configuration leases. Known missing STARTTLS
uses TlsRequired, outbound size limits use OutboundTooLarge; opaque transport/TLS
errors remain Network, without guessing a certificate-specific cause.

## Tests (synthetic data only)

From the workspace root:

```sh
cargo test -p fb-plugin-smtp
cargo clippy -p fb-plugin-smtp --all-targets -- -D warnings
cargo fmt -p fb-plugin-smtp --check
```

The minimal SMTP fake in `src/tests/fake.rs` binds only `127.0.0.1` on a random
port. It generates an ephemeral test certificate in memory, actually negotiates
STARTTLS or implicit TLS, checks synthetic authentication and captures MIME for
assertions. It uses testkit's exact-port loopback policy, credential store, audit
sink, fixed clock and core bridge. Certificates remain validated in these tests,
including wrong-host and untrusted-chain rejection. The SMTP endpoint/root-cert
injection exists only under `cfg(test)`, with no production feature to enable it.
No real DNS resolution, mail account or outside SMTP connection is used by tests.

Coverage includes MIME round-trips, additional recipients/attachments, both AUTH
mechanisms, no-STARTTLS refusal, cert failures, malformed/oversized responses,
provider rejection, stalled TLS/greeting/AUTH/DATA/final response, slow drip,
input/encoded size caps, credential and company isolation, store/audit failures,
DNS pinning/SSRF guards, malformed headers and configuration, and nested-runtime
use. DNS tests inject deterministic answers without connecting to those addresses.

`tests::reply_limits` holds sockets open after sending an unterminated oversized
reply; it requires rejection in under one second with a five-second attempt
limit and independently observes peer EOF/reset. It covers greeting, pre-TLS
EHLO, STARTTLS response, encrypted EHLO, AUTH, DATA and final acceptance. It also
checks multiline response overflow with a final short, unterminated line.
`transport::connection::tests` exercises exact ceilings and one byte over,
fragment sizes of 1/7/1,024 bytes, malformed framing and infinite generated input
without allocating an attacker-sized fixture. The original newline-terminated
`HugeGreeting` regression remains in the suite.

## Real sign-in later — Jody only

Jody will need a mail account whose provider permits SMTP AUTH, its public SMTP
hostname, submission port/TLS mode, authorized From address, username and an app
password. Some providers/organizations disable SMTP AUTH or app passwords; use
the Gmail or Microsoft 365 OAuth plugin if required. No OAuth app registration
or OAuth scopes are needed for SMTP. Jody enters secrets in the eventual secure
settings UI, never chat, logs, environment snapshots or test fixtures. No real
sign-in or email was attempted for this job.
