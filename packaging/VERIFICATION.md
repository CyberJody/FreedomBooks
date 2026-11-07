# Installer verification — 2026-11-02

The follow-up fixes and current artifacts are recorded in
[VERIFICATION-FIX1.md](VERIFICATION-FIX1.md). The results below describe the
original `1869e9e` artifacts retained as upgrade fixtures.

Worker branch `pkg-installers`, based on `dfix3` at `be0a51f`. Local unsigned
preparation only. No merge, push, publication, signing, real sign-in or VM run.
Tools: v24.22.2; cargo 1.99.1 (5f94df478 2026-08-27); rustc 1.99.1 (58cf8f9c3 2026-10-13); electron-builder 26.18.0; Electron 45.2.0.

## Completed checks

- `APPIMAGE SMOKE OK: 6 checks` on the Debian 13 host under Xvfb with a fresh
  HOME. The AppImage was launched directly, without the extract-and-run test
  option and without disabling Chromium's sandbox.
- `DEB SMOKE OK: 6 checks` in a fresh `debian:13-slim` Docker container. Apt
  resolved runtime dependencies before test tools were installed. The app ran as
  an ordinary user. Uninstall removed the launcher and retained the test book.
- Both final Linux artifacts extracted and verified: runtime Electron notices
  byte-identical to upstream ZIP; THIRD-PARTY-NOTICES present; ASAR/native module
  and executable equal to hardened input; no npm dev modules in ASAR; all five
  security fuses read back correctly.
- `license-checker --production` (programmatic production mode) on each exact
  ASAR input: **0 third-party npm production dependencies** (Linux and Windows).
- Windows **incomplete developer shell**: notices, five fuses and PE-embedded
  ASAR integrity hash verified. This is not a runnable app or installer.
- Actual Linux `makensis` preprocessing refused the missing Windows fb-node
  module with the intended guard message and emitted no installer. The builder
  wrapper independently refused the same incomplete shell.
- `LICENSE REJECTION OK: 3 corruption checks`. Altered Electron LICENSE,
  missing Chromium notices and missing THIRD-PARTY-NOTICES were each refused.
- Release verification refuses the missing project LICENSE; no project LICENSE
  was added. It will be included and required once publication creates it.
- The existing packaging fuse test: **1 passed, 0 failed**.
- Workflow structure, Python/JavaScript/shell syntax and unsigned/signing-secret
  placeholder refusal checked locally. GitHub Actions was not run.
- Full `scripts/gate.sh`: **exit 0**; **1,489 passed test executions,
  0 failed, 2 ignored** across workspace and JSON-feature variants.
  Counts include repeated feature-variant executions, not unique test names.
  The workspace run contributed 1,379 passes; the additional
  JSON-feature run contributed 110 passes.

```text
SCREENS MATCH — 512 screens, 0 with differences
gate: clippy, check_json_features.sh (workspace tests + JSON feature unification), core isolation and screen diff passed
```

## Local artifacts

Ignored build outputs in `packaging/dist/`; matching `.sha256` files are beside
them. None are published or committed.

| Artifact | Bytes | SHA-256 |
|---|---:|---|
| `FreedomBooks-0.1.0-d1-x86_64.AppImage` | 126,015,992 | `3954c32a4ad0b686b0bf574fa41c8dacb71b3e044f0570eb6c7b10a6a83753c2` |
| `FreedomBooks-0.1.0-d1-amd64.deb` | 108,650,188 | `f127f1c54177acf01bcc17a478e0750bd145188a146bde15556e73f6b4bd07d2` |

Eight smoke screenshots are retained in `.work/appimage-smoke/` and
`.work/deb-smoke/` (first run, Home, locked, Home after restart). First-run/Home
AppImage screenshots and Debian Home after restart were visually inspected.
These are test evidence, not store-listing screenshots. No preview HTTP server
is needed for this desktop job.

## Left for integration and machine testing

No `FreedomBooks-Setup-<version>.exe` and no Windows fb-node module were built.
See `WINDOWS-BLOCKERS.md` for the reproduced MinGW/napi, MSVC/OpenSSL and Unix-only
attachment-storage blockers. The prepared NSIS source and Windows CI path wait
for that core port; CI's Windows job and dependent release cannot currently pass.
No security storage rule was weakened to force the build.

Jody or the lead must perform every Windows 10/11 and Linux Mint 22.x click-through
check in `INSTALL-TESTING.md`, including Windows user-only/no-UAC installation,
native module load, shortcuts/taskbar, DPAPI/password behavior, offline operation,
upgrade and uninstallation. Actual signing and release CI remain unrun.

Dependency-review carry-forward: `npm audit --prefix packaging` reports
19 moderate development-tool findings (0 high/critical), arising from the
glob/minimatch chains in packager, builder and license-checker. The root advisories
have no fix reported by npm; its direct-package suggestions are breaking
downgrades. No automated downgrade was applied. These tools do not ship in the
ASAR; the production-input audit above remains 0 third-party npm dependencies.
The full local audit is retained in `.work/npm-audit.json`.
