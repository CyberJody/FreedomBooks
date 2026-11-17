# Problem log and panic boundary

## Where the log is

| Run | Folder |
|---|---|
| Installed app on Linux | `$XDG_STATE_HOME/FreedomBooks/logs` (default `~/.local/state/FreedomBooks/logs`) |
| Installed app on macOS / Windows | Electron's `logs` path for the app |
| Development (`FB_DESKTOP_USER_DATA` set) | `$FB_DESKTOP_USER_DATA/logs`, inside the isolated profile |

`freedombooks.log` holds JSON lines. It rotates at 1 MiB and keeps two older files (`.1`, `.2`).
The folder is created 0700 and the files 0600. Report a problem shows the folder and has two
buttons: **Open log folder** and **Copy diagnostic summary**. The summary holds the build, book
format, operating system and the times and kinds of the last five problems. It holds nothing
from the log itself.

## What an entry holds

Each file starts with a header line that names the build. Each entry has these fields:

- `t`: the time.
- `build`: the shell version and commit.
- `schema`: the number of migrations this build knows. An open book is always at that version.
- `source`: `rust` or `main`.
- `op`: the bridge call or main-process step.
- `route`: the screen's route name, never its path or query.
- `kind`: `panic`, `error:IntegrityError`, `uncaughtException`, and so on.
- For a panic: `message`, `location` and `backtrace`, plus what was done about it (`outcome`) and
  whether the books are still open (`book`).

The log never holds book content, secrets or personal paths. `crates/fb-diag/src/scrub.rs` is
the single place that decides this. Messages keep only their first clause. Rust puts the error
value after the first `": "`, and that value is where stored data ends up, so it is always
dropped. In the clause that is left, quoted text, digits, path- and address-shaped words and long
tokens are masked, and URL and HTTP credentials are removed. Locations and backtrace frames keep
code coordinates only. One limit remains: a `panic!` whose first clause formats book words in
would still leave those words. The location is recorded, so the message is never needed to find
the code.

## What a panic does

Every fb-node entry point runs inside the boundary in `crates/fb-node/src/boundary.rs`, on the
main thread and on worker threads alike. A panic becomes an `FB_PANIC {closed, was_open, saved,
op}` error instead of aborting Electron.

- **The transaction in flight is rolled back.** `Book::write` and `Book::read` hold a rusqlite
  `Transaction`, which rolls back in `Drop`. Their callback scopes reset first through their own
  `Drop` guards. `Book::idle_after_unwind` checks the result.
- **Earlier transactions of the same request stay committed**, as they do when a handler returns
  an error. The book's commit counter says which case it was (`saved`: `none`, `partial` or
  `unknown`), and the owner is told.
- **After a call that can write the books or change the session, the books are closed**
  (`App::lock`). That drops the session, the form tokens, the reviews and the key. The shell then
  reopens the books the way startup does if this computer can; otherwise it shows the unlock
  screen. After a read, the books stay open if the connection is idle.
- **The `APP` lock is not poisoned**, because the guard is held outside the catch. If the lock is
  ever poisoned, the books are closed before the lock is used again.

To test the boundary, build with the `panic-test` feature (`FB_PANIC_TEST=1 node
electron/scripts/build-native.cjs`). That is a debug build: optimized builds refuse the feature at
compile time. Then set `FB_TEST_PANIC=render@company_home,write@company_new` (see
`crates/fb-app/src/test_panic.rs`).

## A failed form post

`App::run_desktop` handles a form post that fails after spending its single-use token. The form
comes back as `retry`, keeping what was typed and showing a plain message. It gets a fresh token
only under all of these conditions:

- the post spent a live token;
- nothing was committed;
- the session is unchanged;
- the route does no work outside the book (network sends, connections, backups, settings and
  desktop pages).

Otherwise no token is issued, and the message says to reopen the page and check before trying
again.

## Build identity

`crates/fb-node/build.rs` stamps these values into the native module:

- the commit and whether the tree was dirty;
- the date (`SOURCE_DATE_EPOCH` if set);
- the rustc version.

`electron/scripts/build-native.cjs` supplies exact values. Without git metadata, the commit is
`unknown`. The identity also lists the features compiled in and the migration count. It appears
on the unlock screen's version line, on Report a problem, in the log header, and in each backup's
`app_version` text. That field is free text in a `deny_unknown_fields` manifest, so older and
newer versions still read each other's backups.
