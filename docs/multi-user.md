# Multi-user host: sessions (step 1)

Status: step 1 of host mode, **in-process only**. Nothing here listens on a network or changes the
desktop. The owner decision of 2026-10-30 is that one computer hosts the books
and others connect to that application, never to the book file. This page refreshes the November 4 design
(`design/MULTI-USER.md` outside the repository) against the current code and records what step 1 built.

## What step 1 is

`fb_app::multiuser` lets several people use one open book at the same time, each in their own session:

- `SessionHost` owns the existing `App`: the open book and its single connection, book protection and
  keys, the connection key, the network process, templates, backups configuration.
- Each person's **session** keeps what belongs to that person: identity and MFA progress, the session
  generation, form tokens, review maps, messages, company frame, dialog tickets, pending sign-in
  callbacks, AI grants, recovery authorization and the per-person half of the Backups page state.
- A request swaps that person's state into the `App`, runs on the same code the desktop runs (the same
  authorization, accounting rules and audit), and swaps it back out. The actor is always the session's
  verified user; nothing in a request can choose it.
- `SharedHost` puts a bounded FIFO queue in front of the host. One request touches the books at a time, so
  accounting writes stay serialized on the one connection exactly as today. Waiting requests hear their
  position and what holds the books, and get "busy" after a bounded wait rather than at once.

The standalone desktop (fb-node's single `App`) does not go through the host and behaves as before.
The transport contract for the next step is the `Backend` trait in `crates/fb-remote-proto/src/backend.rs`.

## Lifecycle and scope

| Event | Affects | What happens |
|---|---|---|
| Sign in | that session | Same Argon2id + MFA sign-in as the desktop. Failures are limited per session (as on the desktop), per user name across all sessions (5 failures, then 60 seconds for that name), and second-factor codes per verified user across all sessions (5 wrong codes, then 60 seconds for that user, even in a new session). |
| Sign out, Lock, idle | that session | Its forms, reviews, dialogs, sign-in callbacks and grants expire. The book stays open for everyone else. |
| Session ended (disconnect, "log off") | that session | As sign-out, and the session is removed. |
| An administrator changes a user (disable, role, password, memberships) | that user's sessions | They are signed out with a notice. The acting administrator is signed out too, as on the desktop. |
| Restore, "Use this as my books", reopen, password or key change | every other session | Runs while it holds the only turn. If it changed the book, every other session is signed out with the reason; if it failed without changing anything, nobody is affected. Creating a backup signs nobody out. |
| Close the books on the host | everyone | Every session ends; the book's key leaves memory. |

**Lock.** In a hosted book, "Lock" from a client (button, OS lock, suspend, idle) signs that person out.
It never closes the book for the others. Closing the book is a host action and ends every session. On the
standalone desktop, Lock keeps today's meaning: sign out, and close the file too when the book's
protection policy says so ([Desktop users](desktop-users.md)).

**Maintenance surfaces** (book protection, recovery kit, restore, the Backups pages and backup export)
are available only to the host computer's own session (owner decision 2026-11-16). `Policy::remote_maintenance`
exists for tests and stays `false` in every product configuration.

**First-owner setup** stays at the host computer: a remote session cannot be opened until at least one
active user has a password; the enrollment action is refused to remote sessions; and if the host switches
to books nobody has enrolled in, every remote session ends (and any remote request on such books ends its
session).

## Concurrent work on the same record

- Each form remembers the host's write sequence when its page was rendered. A save through a record's
  route (any route with an id besides the company) is refused if another person saved that record after
  the page was rendered: "*Name* changed this record after you opened it, so nothing was saved. Open it
  again to see the change, then make yours." A person's own saves never make their own other forms stale.
- Company-level pages (setup, numbering, invoice preferences, company documents) and the typed document
  commands keep relying on the core's `expected_revision` checks, which already refuse a stale revision.
- Form tokens stay single-use per session. An exact repeat of a completed post (same token, path and
  fields), for example a client resending after its connection dropped, gets the first answer back and
  is not run again. Any other reuse of a spent token is refused. Another session cannot replay it. The
  remembered answer belongs to the person and identity generation that made it: after any sign-out,
  sign-in, MFA cancel, roster change or recovery from a failure it is gone, and an answer to a post that
  itself changed who is signed in is never remembered. Only ordinary accounting posts on a company's
  pages are remembered at all: identity, user and role, MFA, desktop settings, book protection and
  recovery key, backups and restore, the assistant, company settings, payment, email and bank-connection
  posts never are (their answers can carry secrets or depend on a proof such as recovery authorization),
  nor is an answer that opens an outside address. Before reuse the route's own authorization (its role,
  company membership and active account, as when the post ran) is rechecked; if it fails, the resend
  gets the ordinary "open the screen again" answer.
- Only routes naming an existing record (`invoice_id`, `record_id` and the like; every route argument is
  classified and a test walks the route table) take part. Forms that create a record do not conflict.
- Write sequences and session generations are allocated once per process; the local person keeps their
  own generation. Starting or stopping hosting withdraws the local person's open forms for existing
  records (the host's record of others' saves does not carry across): submitting one says "Hosting
  changed while this record was open, so nothing was saved. Open the record again ...". Other forms
  (new records, settings) are kept.
- Running out of identities or write stamps (about 2^40 sign-ins in one session, 2^24 sessions or 2^64
  saves in one process) never wraps or reuses: a session at the end of its generations is signed out and
  ended (`Evicted`), new sessions and record forms are refused with "FreedomBooks must be restarted
  ...", and a desktop handed back with nobody signed in refuses every sign-in until the app restarts.
- Permission is checked under the turn, at execution, against the book: active user, role, enrolled
  password and company membership. A request queued before a demotion runs with the demoted rights.

## Roles and passwords

Owner decisions of 2026-11-16: the roles stay **Administrator, Bookkeeper and
Read-only** ([Desktop users](desktop-users.md)), replacing the November 4 list with a "regular user"; every
user password has at least 8 characters, local and remote, with no blank passwords.

## Shared and per-person state

`multiuser/state.rs` destructures `App`, `Open` and the session state without `..`, and the Backups
host does the same for its own fields, so a new field does not compile until it is classified as per
person (swapped for each request) or shared, with the reason written next to it.

## Known limits of step 1

- No listener, pairing, encrypted transport or client. The gate keeps fb-app and fb-node free of any
  HTTP/TLS stack, so the network side runs as its own process or crate boundary.
- Reads are serialized with writes (one turn at a time). A read pool is later work.
- Native dialogs (backup folder, restore file, attachments) still return tickets to the local Electron
  main process; a remote client needs its own file transfer.
- The ChatGPT and Gmail sign-in jobs in fb-net are keyed by company. The book host records which session
  generation began each one: only that session polls or cancels it, and nobody else can start another
  for that company until it ends, its owner signs out, or it is 10 minutes old. A second person sees
  "Someone else is connecting this company's account right now".
- The AI loopback listener stays bound to the host desktop's own session.
- Stale-edit detection is per route-addressed record. A save that changes a record through another
  record's page (for example a payment changing an invoice's balance) relies on the core's revision and
  balance checks.
