# Remote access

One computer hosts the books; other computers connect to that host's FreedomBooks, never to the
book file. Design and threat model: [remote-access-design.md](remote-access-design.md). How to host,
pair and revoke: [hosting.md](hosting.md) (Settings › Host these books…, Administrator only, off by
default). There is no headless host until it is built with the decided key custody (TPM-sealed where
available, else a password at each start).

## Pieces

| Piece | Where | Links |
|---|---|---|
| Wire protocol, dispatcher, route policy, retry deduplication, sign-in throttle, audit, gateway link | `crates/fb-remote-proto` | no network/TLS stack (gate-checked) |
| Session backend over fb-app's multi-user `SharedHost` | `crates/fb-host` | no network/TLS stack (gate-checked) |
| TLS gateway (host) and TLS client (connected computer), pairing, device registry, limits | `crates/fb-remote` (`fb-remote gateway` / `fb-remote client`) | rustls; never the book (gate-checked) |
| Client mode in the desktop | `electron/main/remote-client.cjs`, `remote-mode.cjs` | the renderer is unchanged |

The book process starts `fb-remote gateway` as a child (fixed path, empty environment, stdio
frames only). The desktop starts `fb-remote client` the same way in client mode.

## Using client mode (development)

1. Every build has the `fb-remote` helper beside the native module (`node electron/scripts/build-native.cjs`);
   `FB_REMOTE_HELPER` may point at another build in development.
2. On the host, an Administrator turns hosting on and chooses *Pair a computer…* (hosting.md). For
   development without a second app there is also `cargo run -p fb-host --example dev_host -- <empty
   dir> <path to fb-remote>`, which prints an address and a code (synthetic book and passwords only).
3. On the connecting computer: **File › Connect to a FreedomBooks host…**, enter the host address,
   the code and a name for this computer. The pairing is sealed by the OS keyring (Linux
   `basic_text` is refused). Then sign in with your own user name and password on the host's
   sign-in screen.
4. A bar at the top of every page and the window title say which host you are connected to; the
   bar turns amber while reconnecting. **File › Disconnect from host** returns to this computer's
   own books. A computer with no books of its own that is paired starts connected.

Lock, idle and suspend on the connected computer sign **that person** out on the host; the host's
books stay open for everyone else. The page is replaced at once; if the host cannot confirm the
sign-out within a few seconds, the connection is closed (the helper process, and the session with
it, is stopped), so reconnecting needs a sign-in. Disconnecting and quitting sign out too.

## What a connected computer cannot do

Refused by the host before the session layer, and never offered by the client: unlocking,
creating or restoring books; book protection; the recovery key and Emergency Kit; "Use this copy
as my books"; backups; attachments (file upload is not built); AI connections; bank-feed, Gmail
and Stripe connection sign-in or removal (their credentials live on the host); sending or drafting
an invoice email; choosing a bank-rule file. Every screen must be classified in
`crates/fb-host/src/routes.rs`; anything not classified there is refused. Printing prints the
host's print view on the connected computer's printer. CSV exports save through the connected
computer's own Save dialog.

## Checks

- `cargo test -p fb-remote-proto -p fb-remote -p fb-host` — unit tests, the gateway end to end with
  a synthetic backend (`crates/fb-remote/tests/remote.rs`), and two computers plus the host user on
  one synthetic book through the real session host (`crates/fb-remote/tests/real_book.rs`).
- `npm --prefix electron test` — includes `electron/tests/remote.test.cjs`.
- `electron/scripts/smoke-remote.cjs` — real Electron as a thin client of the development host
  (isolated Xvfb display and profile; see the script header), then `smoke-hosting.cjs`: two real apps,
  the host turning hosting on, pairing, the second app signing in, both working on one book, lock,
  revoke and stop.
- `cargo test -p fb-node` — the desktop as the host's local session with the real gateway, sealed
  record, rollback refusal and repair (`crates/fb-node/src/hosting_tests.rs`).
