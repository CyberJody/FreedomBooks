# Remote access: design refresh (2026-11-16)

Status: design for the `t3/remote-access` branch (base 5a8c4fd9). It refreshes the approved
`design/MULTI-USER.md` (2026-11-03, decisions 2026-11-04) against the current code and the council
synthesis of 2026-11-15. Items marked **Owner decision** are not settled by this document.

## 1. What changed since MULTI-USER.md

| MULTI-USER.md said | Current code / council | Consequence |
|---|---|---|
| `fb_user` has no passwords; roles to be added | Named users with Argon2id hashes, `Role::{Administrator, Bookkeeper, Readonly}`, `Permission::{Read, Write, Administer}`, MFA, per-path authorization (`fb-app/src/identity.rs`) | Remote authorization reuses `authorize_path` / `authorize_command` unchanged. The amended "regular user" role is not implemented; current code has Readonly. Remote access uses whatever roles the book has. |
| Blank passwords allowed per user (amendment) | `password_hash` refuses passwords under 8 characters; no blank-password sign-in exists | Nothing to inherit. If blank passwords are ever added, remote sign-in must refuse them (flagged, section 9). |
| `fb-host` = fb-app + axum/hyper + rustls in one binary | fb-app and fb-node deliberately link **no HTTP or TLS stack**; all network code runs in the separate `fb-net` process (`crates/fb-net/src/main.rs:1-4`) | The host boundary is a **separate network process** (`fb-remote`), never TLS inside the book process. See section 2. |
| One `Mutex<App>`; per-session state to be split | Still true at 5a8c4fd9; the split is the parallel multi-user task ([multi-user](multi-user.md)) | The boundary is written against a `Backend` trait; the real adapter waits for that task's `INTERFACE.md`. |
| Loopback AI listener exists as precedent | `electron/main/ai-transport.cjs`: exact numeric loopback Host, bearer, no Origin/Fetch-Metadata, size/concurrency caps, session-generation check after work | Its hardening patterns are reused (caps, generation check, generic refusals that carry no book text). Its socket is **not** widened or rebound. |

## 2. Shape

```
 client computer                                  host computer
 ┌──────────────────────────┐                     ┌──────────────────────────────────────────┐
 │ Electron renderer         │                     │ book process (owns the book and its key) │
 │  (sandboxed, fb://app)    │                     │  desktop: Electron main + fb-node        │
 │        │ typed IPC        │                     │  headless: fb-host service binary        │
 │ Electron main (no book)   │                     │        ▲ JSON lines over stdio pipes     │
 │        │ JSON lines/stdio │   TLS 1.3, mutual   │        │ (child process, fixed path)      │
 │ fb-remote client  ────────┼─────────────────────┼──► fb-remote gateway                     │
 │ (device key, host pin)    │  ALPN fb-remote/1   │   (TLS, pairing, device registry,        │
 └──────────────────────────┘  LAN / tailnet only │    rate limits, connection audit)        │
                                                  └──────────────────────────────────────────┘
```

- **`fb-remote-proto`** (no TLS, no network): wire types, length-limited framing, the host
  dispatcher (`Dispatcher`) that maps typed requests onto a `Backend` (the session API), the remote
  route policy, retry deduplication, the sign-in throttle and sign-in audit. Linked by the book process.
- **`fb-remote`** (rustls + ring, never links fb-app or fb-db): binary with two modes.
  `gateway` terminates TLS on the host and forwards authenticated requests to its parent over stdio.
  `client` runs on the client computer as a child of Electron main and owns the TLS connection there.
  Both sides are one tested Rust implementation of the pinning and pairing rules.
- The gateway never sees the book or its key. The client child never sees a book. A parser bug in
  the TLS-facing process reaches only the typed requests the dispatcher already polices, and
  application sign-in is still required.

**Host modes, one code base.** Desktop hosting: *Settings › Host these books…* (Administrator only,
off by default) starts the gateway child from Electron main; the book stays in the existing
`Mutex<App>`/session layer. Headless: `fb-host` (fb-app + `fb-remote-proto`, no Electron) opens the
book, spawns the same gateway and serves until stopped. The headless binary needs the multi-user
session split to be useful and an owner decision on unattended key custody (section 7).

## 3. Certificate model: mutual TLS with pinned self-signed certificates

**Choice: mutual TLS.** Each host and each client device generates its own key pair and a
self-signed certificate. Neither side uses a CA; each pins the SHA-256 of the other's whole
certificate (DER) at pairing. With 100-year self-signed certificates that are never reissued, a
certificate pin and a public-key (SPKI) pin are equivalent today; if certificates are ever renewed
under the same key, switch to SPKI pins. Below, "pin" means the certificate pin.

Why mutual TLS over "pinned host certificate + per-device pairing secret":

1. **Unpaired and revoked devices are refused in the handshake.** The gateway's client-certificate
   verifier checks the live registry, so they never reach frame parsing, sign-in or any book code.
   With a bearer secret, every unpaired connection reaches the application parser first.
2. **No bearer credential is ever transmitted, logged or stored on the host.** The host keeps only
   public pins: a copied device list grants nothing. A device secret, by contrast, is a password
   replayable by anyone who reads it from host memory, a log or a backup.
3. **Sessions bind to the device.** A host session id is valid only on a TLS connection presenting
   the same device certificate, so a session id copied off one client is useless from another.
4. Cost: two small custom rustls verifiers (pin match, then the normal TLS 1.3 signature
   check). Tailscale `serve` / TLS-terminating proxies cannot sit in front of the host. That is
   intended: the tailnet is only the path, not the authentication.

Settings: TLS 1.3 only, ring provider, ALPN `fb-remote/1` required on both sides, Ed25519 keys,
SNI ignored (pins replace names). Device private keys are sealed by the client's OS store
(Electron `safeStorage`, the same rule as the AI connection files; `basic_text` refused). The host
identity is supplied to the gateway by its parent (desktop: sealed by `safeStorage`; headless: see
section 7) and is never written by the gateway.

## 4. Pairing

1. An **Administrator signed in on the host** chooses *Pair a computer*. The host shows a one-time
   code of 16 Crockford base32 characters (80 bits, `XXXX-XXXX-XXXX-XXXX`) and the host's short
   fingerprint. The code lives 10 minutes and admits one device. An address that sends 5 wrong
   proofs is refused for that code (counted per exact address, so a neighbour in the same IPv6
   /64 cannot use up the new computer's attempts); the code itself is destroyed after 50 wrong
   proofs from all addresses together, so one noisy peer cannot cancel it for the computer being
   paired. Minting a code is the Administrator's approval and is audited with their name.
2. On the client, *Connect to a FreedomBooks host* asks for the host address, the code and a
   computer name. The client connects with its fresh device certificate and records the host's
   certificate pin from the handshake (not yet trusted).
3. Within 5 s of the handshake the client sends `Pair{device_name, proof}` with `proof = HMAC-SHA256(code, "fb-remote pair client" ‖ host_pin ‖ device_pin)`.
   The gateway recomputes the proof with **its own** pin and the certificate actually presented.
4. On success the gateway registers the device pin and replies with
   `HMAC-SHA256(code, "fb-remote pair host" ‖ host_pin ‖ device_pin)`. The client verifies it
   before pinning the host.

An active man-in-the-middle must present its own certificate to at least one side, so the two
pins in the proofs differ from what the honest side computes and both proofs fail. The MITM could
try to brute-force the code offline from one proof, but 80 bits within a 10-minute single-use
window is out of reach. The proof is a plain HMAC keyed by the code: that is safe only because
the code has 80 bits; a shorter code would need a PAKE. Wrong/expired/used codes, unknown pins and
pin mismatches are refused with one generic message, rate-limited per source address, and audited.
While a code is live, any LAN peer can see that pairing is open (an unknown certificate completes
the handshake instead of being refused); that is inherent to pairing over the same port.

**Revocation.** An Administrator revokes a device in the host's device list. The book process
applies it first, without waiting for the gateway: the dispatcher refuses anything from the device
from then on, ends its sessions, drops its stored answers and writes `device.revoked` with the
Administrator's name. Then the gateway is told; it closes the device's live connections, refuses
its certificate and writes the registry atomically (a fresh 0600 temporary file, renamed; the
folder is kept 0700). Its answer is handled whenever it arrives, also after the host stopped
waiting (15 s): `device.revocation_confirmed`, or `device.revocation_unsaved` with the reason when
the registry could not be written (a full or read-only disk), in which case the write is retried
on every later message to the gateway (`device.revocation_saved`). Until the gateway has confirmed
and saved it, `Revocation.unsaved` (and `Dispatcher::revocation`) says so and the host's screen
must keep showing it: a restart of hosting before then would let the device back in. Revoking the
same device again answers "already revoked" with whatever is still outstanding. The registry MAC
has no counter, so putting back an older registry file (with the device still active) is not
detected by the gateway alone: the host's sealed record must keep the revoked ids (section 10).
Re-pairing needs a new code.

## 5. Protocol

Frames: a 4-byte big-endian length plus UTF-8 JSON. Client requests up to 1 MiB, responses up to
8 MiB, stdio frames the same. Unknown fields and duplicate keys are refused (`deny_unknown_fields`).
One request is in flight per connection; a client may open more than one connection.

| Request | Pre-sign-in | Maps to |
|---|---|---|
| `Hello{resume?}` | yes | create, or resume this device's session (`Backend::open_session`) |
| `Render{target}` | sign-in/MFA paths only | `Backend::render(session, target)` |
| `Run{request_id, command}` | sign-in, second factor and cancel on the sign-in screen only | `Backend::run(session, command)`, deduplicated (exports are not) |
| `Query{query}` | no | `Backend::query(session, query)` |
| `SignOut` | yes | `Backend::sign_out(session)` |
| `Ping` | yes | liveness |

Sign-in uses the host's own sign-in and MFA screens through `Render`/`Run`: the session layer stays
the only place passwords are checked. The dispatcher throttles sign-in submissions per device
(5 failures, then a minute's wait, doubling to 15 minutes; one sign-in checked at a time per
device, so parallel connections cannot race the count) on top of fb-app's own limits (per user
name, and second-factor failures per verified user), and audits each sign-in success, failure and
sign-out with the device name. There is no per-address password throttle: per-address limits
apply to connections (section 8).

**Remote route policy (deny by default).** A screen or form is served only if its route is
classified `Allowed` in `crates/fb-host/src/routes.rs`; a test fails when any route in the route
table or any desktop page is missing there, so a new screen is unreachable remotely until someone
classifies it. Host-only (refused before the backend): unlocking/creating/restoring books, book
protection, recovery key and Emergency Kit, *Use this copy as my books*, backups, attachments, AI
connections, bank-feed connection and removal, the host's problem log and the unused Django
sign-in routes; a fixed list in `fb-remote-proto` refuses the same paths and odd spellings as a
second layer. Form actions refused before the command runs, on any route: Gmail and Stripe
connection actions, invoice email send and draft (they hand a PDF to the host's window), and
choosing a bank-rule file. Outcomes carrying `desktop`, `desktop_keys`, `invoice_email_pdf` or
`external` are still withheld as a backstop, and that is audited (`policy.outcome_withheld`),
because it means the policy missed a command. CSV exports (`Outcome::Export`) are forwarded; the
client suggests only a cleaned file name (last path part, safe characters, at most 100, `.csv`)
in its own Save dialog and refuses a chosen name that does not end in `.csv`.

**Retry deduplication.** Every `Run` carries a client-generated UUID `request_id`. The client child
resends it unchanged while it reconnects (30 s), and the desktop reuses it when the person submits an
identical form again after a lost answer. A client crash loses the id; then the single-use form token
and the session layer's duplicate-post replay still stop a second post. The dispatcher keys stored
answers by `(device, request_id)` and keeps with each a fingerprint of the session token, the
signed-in user's id and the command text. A repeat with the same fingerprint while the original
runs waits for it; after it finished it gets the stored answer and runs nothing; any difference
(another session, another user, signed out, another command) is a conflict and runs nothing.
Answers belong to their session and are dropped when its sign-in changes, it signs out, idles out,
is evicted or ends, so a stored answer is never handed to the next person on the same computer.
Answers live 15 minutes, at most 1024 and 16 MiB per device and 128 MiB in all (oldest dropped
first). Read-only exports (`ExportHistory`, `ExportReport`) are never stored. A dropped connection after commit therefore never
posts twice. The cache lives in the book process, so a gateway restart keeps it. A host restart
loses it; then the existing single-use form tokens refuse the replay ("open this page again and
check") and document request keys/revisions answer with the saved document. A `request_id` reused
with a different command is refused.

## 6. Sessions, lock, idle and restart

- Each remote session is independent and needs its own sign-in. A remote user's idle timeout
  (15 minutes, as on the desktop) signs out **that session** only.
- The host operator locking the desktop (Ctrl+L, idle, suspend) currently closes the book in the
  single `App`; while the desktop is hosting, that would end every remote session. Target behaviour,
  once multi-user lands: in hosting mode the desktop's lock signs out **the desktop's own session**
  and leaves the book open for others; a deliberate *Stop hosting* or quit ends remote sessions with
  "The host stopped sharing these books." Book-wide maintenance (backup/restore/rekey/migration)
  pauses remote requests with a busy answer instead of failing them.
- Host restart: the gateway restarts with the same pinned identity and registry, so paired devices
  reconnect; remote users sign in again (sessions are in memory by design).
- Client disconnect: requests in flight are retried on a new connection with the same
  `request_id`. Forms are never resubmitted by the user; the window shows "Reconnecting to <host>".

## 7. Unattended host key custody — Owner decision

Recorded decision 2026-11-04 (4) was "sealed in the OS keyring (0600 key file where there is none),
restarts unattended". The council (Opus, Sol, Astra) warns that a headless host often has no unlocked
keyring, and a 0600 file next to the book is weaker than today's desktop custody. Options, from
strongest to most convenient:

| Option | Unattended restart | Protection if the disk is copied |
|---|---|---|
| A. Book password typed at every service start (systemd-ask-password / console) | no | full |
| B. OS keyring of a logged-in service user (libsecret/Keychain/DPAPI) | only with an unlocked keyring session | good |
| C. TPM-sealed key (systemd-creds `--with-key=tpm2`, Windows DPAPI machine scope) | yes | good against disk theft, not against the running machine |
| D. 0600 key file beside the book | yes | **none** — equivalent to the built-in key |

This branch does not implement headless key custody and does not silently pick D. Desktop hosting
needs none of this: the desktop already holds the unlocked book. Recommendation: desktop hosting
first; then headless with C where available, A otherwise, D refused unless the owner explicitly
accepts it per install.

## 8. Network scope and threat model

The listener is **off by default** and binds only an address the Administrator chooses. It refuses
unspecified (`0.0.0.0`, `::`) and public addresses: allowed are loopback, RFC 1918, link-local,
IPv6 ULA and the Tailscale CGNAT range 100.64.0.0/10. The client refuses public host addresses the
same way. There is no public exposure, port forwarding or UPnP.

| Threat | Mitigation |
|---|---|
| Stranger on the LAN/tailnet | handshake refused without a pinned device cert; pairing needs a live 80-bit code |
| Stolen client laptop | device key sealed by the OS; user password + MFA still required; Administrator revokes the device |
| Device key copied off a client | user sign-in still required; revocation; audit shows the device |
| Active MITM / spoofed host | mutual certificate pins; pairing proofs bind both pins |
| Password guessing | per-device throttle (one check at a time) plus fb-app's per-user-name and second-factor limits; audit |
| Connection flooding | the certificate is checked first; only connections that have not shown a paired certificate are pending, and the pending pool (4 per address, 8 per IPv4 address or IPv6 /64, 32 in all) never refuses a newcomer but closes the oldest pending connection of that address, that source, or the source holding the most; a paired computer then counts apart (8 per address, 64 in all); new connections per exact address are rate-limited (burst 10, one per 3 s); 3 s handshake deadline, 5 s for the pairing frame; read timeouts; frame caps. Residual: a machine in the paired computer's own /64 that opens more than 8 connections within one handshake (a few milliseconds, sustained) can still delay it |
| Replay of a captured request | TLS 1.3; `request_id` only returns the original answer to the same device, session and signed-in user |
| Tampering with the host's remote audit | HMAC chain keyed by the book process; MAC'd checkpoint catches truncation and deletion; see section 9 |
| Duplicate posting after a dropped connection | `request_id` deduplication; form tokens; document request keys |
| Remote user reaching host-only powers | route policy + outcome filter; existing role checks |
| Compromised gateway process | cannot open the book or see keys; cannot sign in without a password; the book process re-checks policy |
| Browser-based CSRF / DNS rebinding | not HTTP; a browser cannot speak mutual-TLS framed JSON with ALPN `fb-remote/1` |
| Host compromise | out of scope: the host holds the book by design |

## 9. Other flags

- **AGPL-3.0 §13** (licence decision 2026-10-30): when users interact with a modified version over a
  network, §13 asks that they be offered the Corresponding Source. Thin clients run the same
  released build as the host, so offering the source with each install may already satisfy this,
  but a modified host serving unmodified clients may need a "Source" link on the host's screens.
  This is a flag, not a legal determination.
- **Blank passwords**: if the amendment allowing blank per-user passwords is ever implemented,
  remote sign-in should refuse blank-password users unless the owner decides otherwise.
- **Native printing and attachments** in thin clients: printing renders the host's document page
  and prints it locally (no host file access). Attaching a file needs the bytes sent to the host
  through a size- and type-checked upload request; deferred, and refused remotely until then.
- Device and session columns in the book's audit table would need a book-format change; this branch
  keeps the host audit as a separate log instead (`fb-remote-proto/src/audit.rs`). Each line
  carries an HMAC-SHA-256 keyed with the audit key over its content and the previous MAC, and a
  checkpoint file (`<log>.head`, MAC'd with the same key) records the last line. Detected on open
  and by `verify`: an edited, removed or reordered line, lines rewritten with recomputed MACs
  (without the key), lines removed from the end, a deleted log or checkpoint, and (when the host
  says it has used its key before) both deleted.
  - **Rollback is not detected by the files alone, and it is easy**: the log is append-only, so
    only an earlier copy of the small checkpoint file is needed; cutting the log back to it
    verifies. Any process running as the owner's OS user can keep the previous checkpoint and so
    drop the newest lines (a `device.revoked` among them) whenever it wants. The same holds for the
    device list: putting back the old file makes a revoked computer active again. Until the sealed
    host record exists (section 10), the audit detects accidents and outsiders, not a local
    process, and a revocation does not survive someone restoring the old device file. The host
    closes this by keeping the head in its sealed record and opening with `open_with_head`, which
    requires the log to reach the kept line with the kept MAC (an earlier copy, or a different
    chain even when longer, is refused).
  - **Write failures fail closed.** After any failed write (line, sync or checkpoint) the log takes
    no further lines until it is opened again, and the dispatcher stops remote access at the same
    moment: every request is refused with "remote access is paused", every remote session ends,
    and `Dispatcher::audit_error` / `HostLink::audit_error` give the reason for the host's screen.
    Restarting hosting reopens the log: lines past the checkpoint are accepted when each verifies
    under the key (they cannot be made without it) and the checkpoint is brought up to them, and a
    last line torn by a crash is set aside in `<log>.torn-<time>` and the log cut back to whole
    lines (only when everything before it matches the checkpoint); each repair is recorded as
    `audit.repaired`. A checkpoint at line 0 without a log (a crash on the first start) is an
    empty log.
  - **The audit key** is 32 random bytes the book process makes and seals itself
    (`audit::open_for_hosting` with a `KeyStore`: the OS keyring through the desktop app). It is
    never derived from the TLS key (the gateway, the network-facing process the split does not
    trust, is handed that key), never sent to the gateway, and never in logs, diagnostics bundles
    or problem reports; `Debug` hides it. **Lost key** (keyring reset, key deleted): a host that
    has hosted before refuses to start on the old log (`audit::LOST_KEY`). The Administrator's
    repair, `audit::start_over`, sets the old log and checkpoint aside as
    `<log>.lost-key-<time>` and `<log>.head.lost-key-<time>`, seals a new random key, and starts
    a new log whose first line (`audit.key_replaced`, with the Administrator's name) names the
    old log's last line number and MAC as found; the host shows the Administrator what was set
    aside. The old log can no longer be checked.

## 10. Before hosting is switched on (acceptance list for the next step)

Nothing in the product can start hosting yet: only tests and `dev_host` call `HostLink::start`
and open the audit log. The hosting step (the "Host these books" toggle and the sealed host
record) is accepted only when all of these hold. Items marked *done* were fixed on this branch
after the second security re-check (`review-fable-2.md`, A1–A5).

1. *Done:* an audit write failure fails closed and the next start repairs and records it (A1); a
   revocation is applied and recorded by the book process first, its late answer is recorded,
   and a repeat answers "already revoked" (A2); `open_with_head` checks the kept head's MAC at its
   line (A4).
2. **A sealed host record**, kept by the book process outside the gateway's folder, holding: that
   this host has hosted before (drives `hosted_before` / `Expect`, never whether the files exist);
   the audit head, updated after every append; a counter or digest of the device list, checked
   by the book process at start and after every change; the list of revoked device ids, so a
   restored device file cannot re-admit one.
3. **Audit key**: random, made and sealed by the book process through `KeyStore`, never sent to
   the gateway, never in logs, diagnostics bundles or problem reports (*hook done:*
   `open_for_hosting`; the keyring-backed `KeyStore` is the next step). The lost-key path
   (`start_over`) is offered to an Administrator only, and shown to them with what was set aside.
4. **Start with `open_with_head`** against the sealed head on every start, and refuse to host on a
   mismatch with a message that names the repair action. The repair action itself is
   Administrator-only and recorded.
5. **Revocation in the hosting screen**: show `Revocation.unsaved` (not confirmed, or not saved)
   and keep showing it until `Dispatcher::revocation` reports nothing outstanding
   (`device.revocation_confirmed` / `device.revocation_saved`). Show `audit_error` when remote
   access stopped for the audit log, with the restart that repairs it.
6. **Connection limits**: *done* for the reviewer's cases (oldest-pending eviction, certificate
   first, no trusted-address state, 5 s pairing wait). The remaining case (a machine in the
   paired computer's own IPv6 /64 opening connections faster than a handshake completes) is
   recorded as accepted for the owner's own network, and the user documentation must say so.
7. **Network scope**: the Administrator picks one address from the computer's current interfaces;
   warn when it is in 100.64.0.0/10 and no Tailscale interface owns it; stop hosting and say why
   when that address leaves the computer or the network changes; never offer "all interfaces".
8. **Hosting toggle**: Administrator only, off by default, state kept in the sealed record;
   pairing, device list and revoke need an Administrator session on the host.
9. **Review what the first review could not**: the host-side screens, sealing of the host TLS
   identity, packaging of the `fb-remote` helper at a fixed path, and how book backup and restore
   treat the audit log and device list (a restore must not bring back an older device list).
10. **Evidence on the final head**: full gate, smoke-remote including the two Lock steps, and the
    probes in `review-evidence/fable-probe2/` rerun.

### 10a. Status after the hosting step (`t3/hosting`, 2026-11-16)

"Host these books" is built (fb-node `hosting.rs`, `electron/main/hosting.cjs`, user guide
[hosting.md](hosting.md)). Against the list above:

1. Done before; unchanged.
2. **Done.** The sealed host record is in the encrypted book (migration `0206_remote_host`, book
   format 93): hosted-before (drives `Expect` through `open_for_hosting`), the audit head (moved after
   every line by the audit sink and written into the book by a record writer that waits for the
   lease; a write failure pauses remote access), the paired-computer list's counter (checked by the
   book process before every start and read back after every gateway write) and every revoked id
   (sent to the gateway in `Start` and refused by the dispatcher whatever the list says). The
   head and counter in the record only move forward; the record may lag the files by the lines
   written since the last write into the book (normally milliseconds, longer while a backup holds
   the books), which a rollback cannot exploit beyond those lines.
3. **Done.** The audit key and the list's key are random, made by the book process and kept in the
   record; neither is derived from the TLS key, the audit key never reaches the gateway, and
   `HostRecord`'s `Debug` and the Hosting screen's state carry no key. Lost key, rolled-back or
   damaged log: refused with its kind; the Administrator's repair (`audit::set_aside`) keeps the old
   files under a dated name, says where on the screen, and the new log's first line records it with
   their name.
4. **Done.** Every start opens the log with the kept head; a mismatch refuses to host with a message
   naming the repair.
5. **Done.** Outstanding revocations (listener not confirmed or not saved, or not yet in the record)
   stay on the Hosting screen; `audit_error` and record errors show with *Restart hosting*.
6. Accepted residual documented in the user guide (same IPv6 /64).
7. **Done.** One address from the computer's current private addresses; never all interfaces; a
   warning for 100.64/10 without a Tailscale interface; a network watch stops hosting and says why
   when the address leaves or its interface or netmask changes.
8. **Done.** Administrator only, off by default, on/off and address in the record; pairing, the
   device list and revocation need an Administrator signed in on the host.
9. Built: the host screens, the host TLS identity sealed in the record, `fb-remote` packaged at its
   fixed path and verified in the artifact checks. Backup and restore: the hosting folder (audit log,
   list of paired computers) is not in a backup; a restored book brings back an older record, which
   accepts the newer list on disk and takes its revocations back (tested). **Still needed: the
   independent security review of this step.**
10. See [Host these books](hosting.md).

Desktop routing (request (a)): while hosting, the desktop window is the host's local session
(`SessionHost::host_with_local`); renders, form posts and queries go through `dispatch`, everything
else through `with_session`. Lock, idle and sleep on the host sign out only the host's person; closing
the books or quitting stops hosting and tells connected computers (`HostLink::stop_with`).
