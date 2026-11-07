# FreedomBooks installers

Build only in a FreedomBooks worktree. Node 24, Rust stable, a C/C++ compiler,
Perl, Python 3 and the tools below are developer requirements, never customer
requirements. Output is unsigned and stays local. No publishing command is in
the local build scripts. Project LICENSE creation waits for publication.
The Linux native build requires glibc 2.38 or newer; the intended machines are
Debian 13 and Linux Mint 22.x.

```sh
npm ci --prefix packaging
npm ci --prefix electron
python3 packaging/build-native.py linux
node packaging/installers.cjs linux
python3 packaging/verify-artifacts.py packaging/dist/*.AppImage packaging/dist/*.deb
python3 packaging/test-deb.py packaging/dist/*.deb
python3 packaging/test-appimage.py packaging/dist/*.AppImage
```

Set `TMPDIR` to a writable, private job directory first. Node's binary directory
must be on PATH. `bootstrap-local.py node` downloads checksum-verified Node 24
into `.work/node`; `bootstrap-local.py linux` extracts Xvfb without sudo. Set
`ELECTRON_BUILDER_CACHE` to `packaging/.work/builder-cache` to isolate its tools.
The Debian test uses a disposable `debian:13-slim` container; it installs the
`.deb` with apt before adding headless test tools, runs six checks as an ordinary
user, then checks uninstall preserves the book. Chromium stays sandboxed. The
container needs extra namespace capabilities only for this headless test.

Linux books and key wrappers live in `$XDG_DATA_HOME/FreedomBooks`, defaulting
to `~/.local/share/FreedomBooks`. On first launch, the previous book and key
wrappers in `$XDG_CONFIG_HOME/app.freedombooks.FreedomBooks` move there together
through a private, resumable stage. An existing destination book is retained.
Electron settings and cache keep their existing locations. Files stay 0600,
directories 0700. Cross-filesystem migration copies, fsyncs and verifies SHA-256
before removing each source file.

The `.deb` keeps `chrome-sandbox` at 0755 when user namespaces are available.
On Ubuntu/Mint with AppArmor's userns restriction, it installs and loads
`/etc/apparmor.d/app.freedombooks.FreedomBooks`, granting `userns,` to the
application binary. Removal unloads/removes that profile. Only a disabled
userns sysctl or a failed AppArmor profile load selects 4755, with the reason
logged by the package manager. This follows the
[Chromium AppArmor profile pattern](https://chromium.googlesource.com/chromium/src/+/main/docs/security/apparmor-userns-restrictions.md).
`python3 packaging/test-sandbox-policy.py` covers the decision branches without
changing host policy; actual Mint kernel enforcement still needs a Mint kernel.

The `pkg-flatpak2` manifest needs no extra filesystem permission: Flatpak sets
`XDG_DATA_HOME` inside its application sandbox, so the same code selects its
`data/FreedomBooks` subdirectory. The bundled Flatpak app is packaged, so its
legacy development-only `FB_DESKTOP_USER_DATA` export is ignored. Keep the
packaged-override gate when integrating the branches; do not restore the older
Flatpak branch's unrestricted environment override.

On a headless host without a usable FUSE mount, add `--extract-and-run` to the
AppImage test. This uses the actual AppImage runtime's fallback without changing
Electron's sandbox or fuse settings. It is developer test tooling; customers
should use the `.deb` if their desktop cannot directly open the AppImage.

Windows currently needs the core portability work described in
`WINDOWS-BLOCKERS.md`. After that fix, use the standard Rust/MSVC build tools:

```powershell
python packaging/build-native.py win32
node packaging/installers.cjs win32
python packaging/verify-artifacts.py packaging/dist/FreedomBooks-Setup-*.exe
```

PowerShell does not expand that final wildcard for Python; pass the actual
filename (the CI driver discovers it itself). Both native MSVC and cross-builds
statically link the CRT, avoiding a redistributable prerequisite. The native
module uses Node-API 8. The Windows CI runner is configured to build it on
Windows; it cannot pass until the core is portable. No VM is used by this worker.

Attempted Linux cross-build route (currently blocked; see `WINDOWS-BLOCKERS.md`):

```sh
python3 packaging/bootstrap-local.py llvm
cargo install cargo-xwin --version 0.23.1 --locked --root packaging/.work/cargo-tools
rustup target add x86_64-pc-windows-msvc
python3 packaging/build-native.py win32
node packaging/installers.cjs win32
python3 packaging/verify-artifacts.py packaging/dist/FreedomBooks-Setup-*.exe
```

LLVM, the SDK/CRT cache and cargo-xwin stay in `.work`; Rust's target component
is installed by rustup for the current user. cargo-xwin's upstream terms apply
to its Microsoft SDK download. electron-builder's checksum-verified user-local
Wine toolset is used only to compile the installer/uninstaller. Never use it as
a substitute for the Windows machine checklist. The attempted MinGW route is
not supported here: napi-build's GNU helper requires `libnode.dll`, which is not
the Electron runtime. MSVC cross-compilation additionally hits OpenSSL's Windows
Perl requirement, and the existing core has Unix-only attachment storage that
needs a Windows backend. The native Windows CI route also waits for that core
port. The worker's explicit fallback can be reproduced with:

```sh
node packaging/windows/prepare-shell.cjs
python3 packaging/verify-artifacts.py --incomplete-windows electron/out/FreedomBooks-win32-x64
```

This is a non-runnable developer shell, never a release. The standalone
`windows/FreedomBooks.nsi` recipe and canonical builder configuration both await
a complete native module. Installer creation refuses the incomplete shell.

`FB_RELEASE_VERSION=0.1.0` selects a version for a fresh package. To rewrap an
unchanged already-hardened app while developing installer metadata only, add
`--wrap-only`. It verifies fuses and refuses a version mismatch first.

The packager whitelists the ASAR input, runs `license-checker --production` on
that exact input and rejects npm production dependencies until audited. The
runtime notices are copied unmodified; final artifacts are extracted and checked
against Electron's downloaded ZIP and the hardened input. All five fuses are
read back on both the input and every extracted application binary.
`--release` additionally requires our future project LICENSE. The CI workflow
uses that mode and refuses to release before publication preparation is done.

Desktop entry and PNG/SVG source assets are copied from `pkg-flatpak2` at
`690f14f`. Builder adjusts the executable path and icon filename for its install
layout; the AppImage launcher uses `AppRun`. The icon artwork is unchanged.

Developer references: [electron-builder prepackaged CLI](https://www.electron.build/docs/cli/),
[AppImage runtime](https://www.electron.build/v26/docs/appimage/),
[NSIS options](https://www.electron.build/v26/docs/nsis/), and
[cargo-xwin](https://github.com/rust-cross/cargo-xwin).

## Internal SimpleFIN build

Build `fb-node` with its `simplefin` feature and build `fb-net` without test features.
Pass the native library as `build({native, netHelper, out, version})` to
`packaging/package.cjs`, with `netHelper` pointing to that production `fb-net`.
The packager includes the helper as an executable unpacked resource;
`verify-tree.cjs` checks it against the declared SimpleFIN capability. Omitting
`netHelper` keeps the default package offline. AI remains disabled unless the
separate full `plugins` feature is selected. `FB_SIMPLEFIN=1` also selects the
SimpleFIN-only native build in `electron/scripts/build-native.cjs`.

Test with synthetic providers before delivery. Fake-network helper builds are
for isolated verification only and must not be delivered to users.
