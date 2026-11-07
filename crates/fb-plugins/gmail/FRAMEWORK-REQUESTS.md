# Gmail framework/host integration requests

W1b-ADOPT adopts framework 66859ce without editing any framework files.

## GMAIL-1 — closed: typed sent-message audit metadata

Accepted sends now call `record_document_send_audit(DocumentSendAudit)` with the
validated Gmail id, exact receipt, document ref, scope, actor and injected time.
No duplicate generic success audit is emitted. Tests use MemoryAuditSink's
`document_sends` and FakeCoreBridge's `sent_receipts`; the old receipt-capturing
wrapper is removed (a separate fault-only bridge injects history failures).

The existing provider-owned `record_document_send` bridge call is retained until
wave 4 selects one atomic history/audit owner. The host must not record it again
just because it receives a receipt. Rich audit is metadata support, not durable
atomic persistence. The host must implement the rich sink; its default fails Audit.

## GMAIL-2 — closed: optional object-safe lifecycle and cancellation

`connection_lifecycle` exposes begin/complete/disconnect. Begin accepts JSON
`{"from_address":"sender@example.test"}` in secret ConnectInput fields. Attempts
retain mailbox/client/scope/epoch validation and the shared sender mutex.
Completion uses the context's cancellable framework wait, checks cancellation
before saving and reports scoped Connected status/expiry. Disconnect reports
Disconnected plus Confirmed only for Google 200, otherwise Unknown.

Host encrypted persistence, registry status transitions, enabled/unlocked operation
leases and transactional cancellation/save coordination remain wave 4. An access
expiry alone is not evidence for Revoked/Expired. NeedsSignIn remains evidence-based
on invalid credentials/grants/401, never guessed from a temporary provider failure.

## GMAIL-3 — closed: rich send outcomes

`send_outcome` retains the validated receipt after provider acceptance, including
when history or audit fails (`Accepted` with `recording_error`). Both local recording
steps are attempted once. A lost/malformed final response after the POST is Uncertain;
pre-send validation and known provider refusals are NotSubmitted. There is no retry.
Legacy `send` retains its conservative error-returning compatibility behavior.

Remaining wave-4 gates: durable pre-submission attempt/outbox recovery, atomic
history/audit ownership, Sent inspection/recovery UI, encrypted credentials and
host-wide config/unlock leases. Gmail acceptance is never proof of recipient delivery.
No real sign-in or mail was sent.
