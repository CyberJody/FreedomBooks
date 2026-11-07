# SMTP framework requests — W1b-ADOPT

Framework 66859ce is adopted; no framework files are edited.

## 1 — closed: protocol-neutral pinned endpoint policy

Transport takes PluginContext/PluginHttp and uses `resolve_endpoint` with the
remaining attempt deadline. No provider SystemResolver, HTTPS-shaped policy URL,
or empty-answer sentinel remains. Only returned numeric addresses are connected;
the returned remaining budget bounds the entire SMTP/TLS operation and TLS still
authenticates the original host. Resolver tests inject through PluginHttp itself.

The host must still grant the exact consented SMTP hostname (no wildcard) and bind
host/From/configuration to the scoped operation. The SMTP-specific cfg(test) TLS fake
is retained. No public insecure TLS or endpoint override exists.

## 2 — closed contract, durable host work remains wave 4

`send_outcome` distinguishes pre-DATA refusal from uncertain submission after DATA
starts. Final 250 returns Accepted with an exact no-id receipt, retained even when
rich local audit fails. Lost final response, timeout or cancellation after DATA is
Uncertain, never NotSubmitted. No retry or invented queue id. Legacy send remains
conservative and returns errors for uncertain/recording failures.

Accepted metadata uses DocumentSendAudit (document ref, receipt, scope/actor/time),
not a second generic success audit. SMTP document history remains host-owned, so
no duplicate bridge call was added. Host must implement the rich audit sink.

Cancellation is checked before network/auth/DATA, plus cooperatively while awaiting
I/O (20 ms checks, socket/future dropped). An already-started send cannot be undone.
The 3c3ed28 line/response caps and above-TLS bounded framing are intact and green.

Wave 4 still owns durable attempts/outbox/crash recovery, Message-ID ownership,
atomic audit/history finalization and real unlock/config leases. No exactly-once
SMTP delivery claim is made. SMTP credential save/status/forget remains host-owned;
there is no upstream SMTP password-revocation protocol.

## 3 — closed: safe error codes

Known missing STARTTLS maps to TlsRequired; outbound MIME/input limits map to
OutboundTooLarge. Generic TLS/transport errors remain Network rather than guessing
certificate rejection from an opaque failure. Framework Timeout no longer tells
the user to blindly retry. Certificate verification remains enforced and tested.

No live account, DNS or external SMTP operation was used.
