# Microsoft 365 framework requests — W1b-ADOPT

Framework 66859ce is adopted. No shared framework/core files are edited.

## 1 — closed: rich submission outcomes and typed audit

Graph 202 returns Accepted with the exact timestamp and no invented message id,
even if local audit fails (recording_error remains independent). Post-submission
transport loss/timeouts or malformed/unexpected responses are Uncertain, never
NotSubmitted. Proven preflight and known provider refusal are NotSubmitted. No
automatic send retry exists; acceptance is not recipient delivery.

Accepted metadata uses DocumentSendAudit with document ref, exact receipt, scope,
actor and injected time, without duplicate generic success audit. The host must
implement that rich sink (the default fails Audit). Document history remains
host-only: no provider bridge call was introduced, so the host finalizes it once.

Durable pre-submission attempts/outbox/recovery and atomic history/audit persistence
remain wave 4. The user checks Sent Items before deliberately resending.

## 2 — closed: optional lifecycle and scoped cancellation contracts

`connection_lifecycle` exposes object-safe begin/complete/disconnect. Begin accepts
secret JSON `{}`; the provider client UUID stays constructor configuration.
PendingConnection retains scope/client/generation state; shared clones use the same
provider mutex and attempt generations. Completion uses wait_cancellable and
checks context/captured cancellation before exchange/save. The context carries the
host-issued scoped OperationLease; its cancellation is exercised in tests.

Successful completion returns Connected plus expiry. Local-only disconnect returns
Disconnected plus Unknown revocation, never Confirmed. Registry status persistence,
real unlock/credential-commit coordination, cross-instance/process config leases
and encrypted storage remain host responsibilities in wave 4. Keep one shared
provider instance. Access-token expiry alone is not a sign-in/revocation claim.

## 3 — OPEN: Microsoft numeric loopback registration gate

This plugin retains the framework's exact 127.0.0.1:0 dynamic listener, path and
state checks. Fake tests prove bind/path/redirect consistency, not real Microsoft
acceptance. No registration or sign-in was attempted. Wave 4 must verify the
native/public client configuration for personal and organizational accounts on
Jody's go. No localhost/fixed-port fallback or callback weakening was introduced.
See README for registration checklist and evidence labels.
