# FreedomBooks desktop shell (D1 + D2)

Electron window over the Rust core (ARCHITECTURE layer 5). Local only, not packaged (packaging is stage D).

- `main/main.cjs`: window, the `fb://app/` scheme handler (renders screens through fb-node, serves only
  `renderer/static/`), the fixed IPC call list, lock on idle/suspend/screen lock and Ctrl+L.
- `main/guards.cjs`: CSP, static-file guard, payload checks, sender checks, navigation/window/permission/
  network blocking. `main/keys.cjs`: where the book and its key blobs live (safeStorage, basic_text rule).
- `preload/preload.cjs`: exposes `window.fb` = `books.run`, `books.query`, `screens.render`,
  `files.pickReceipt`, `app.lock`; sends the ported pages' POST forms as `books.run({type:'SubmitForm'})`
  and the first-run/unlock forms as `app.createBook` / `app.unlock`.
- `renderer/static/`: the frozen app's CSS, icons and page scripts, copied unchanged.

Build and run (Node >= 22.12 for Electron 45's installer; the S1 Node 24 tarball works):

    export PATH=$PWD/.tooling/node-v24.22.2-linux-x64/bin:$HOME/.cargo/bin:$PATH
    npm ci && npm run build:native && npm test
    xvfb-run -a -s '-screen 0 1600x1200x24 -nolisten tcp' npm run smoke   # headless end-to-end smoke
    npm start                                                              # a real window

`FB_DESKTOP_USER_DATA=/abs/dir` isolates the data folder (tests). Dependencies are pinned in
`package-lock.json` (electron 45.2.0, playwright 1.64.1 for the smoke only).

## D2

The permanent identity is `app.freedombooks.FreedomBooks` (product name FreedomBooks). The normal data
folder is `<appData>/app.freedombooks.FreedomBooks` on every OS; `FB_DESKTOP_USER_DATA` remains an explicit
test override. Windows sets its AppUserModelId; Linux sets the desktop-file name and WM_CLASS.

Company/account forms, setup/checklist/fill, journals, audit history/CSV, trial balance and Report a
problem use the ported Django screens. POSTs honor each form's action and use typed commands. Audit CSV
uses a native Save dialog through the existing `books.run` method. The frozen empty journal attachment
panel keeps its URLs; actual attachment operations/populated rows belong to D9. PDF printing is later.

Regenerate the fourteen D2 templates with:

    PYTHONDONTWRITEBYTECODE=1 python3 crates/fb-templates/tools/port_d2.py

The extended smoke exercises real account/setup forms, journal Add line, invalid input retention,
editing/posting/reversal, CSV save, the other D2 screens, and D1's hardened-window/invoice checks.
Set `FB_SMOKE_SHOTS` to the project's `staging/d2` directory. It also captures the shared print-note
include from `target/screendiff/rust/print-toolbar-note.html`, so run the screen diff first.

If Xvfb is absent, extract Debian's `xvfb` package into this worktree's ignored `electron/.tooling/xvfb`
and prepend its `usr/bin` directory to PATH. No sudo or machine-wide install is needed.

The known existing core refusal mismatch is documented in `../D2-CORE-GAPS.md`; the screen gate keeps
that case visible instead of changing the core or suppressing its missing sentence.
