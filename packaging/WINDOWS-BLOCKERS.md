# Windows native module and installer status

The Linux worker did **not** produce a runnable Windows application or NSIS
installer. It produced a hardened **incomplete unpacked Electron shell**, with
real Windows Electron binaries, ASAR, product icon, runtime license files,
third-party notices, embedded ASAR integrity and the five fuse settings. The
`INCOMPLETE-WINDOWS-NATIVE.txt` marker and ASAR metadata identify it as a developer
fallback. No Linux `.node` file or dummy native module was substituted.
`installers.cjs` refuses to make an installer from this incomplete input.

## Reproduced build blockers

1. **MinGW GNU ABI:** user-local GCC/MinGW tools installed and the Rust target
   loaded, but `napi` 3.16.0 invokes `napi-build` 2.6.0's GNU helper, which panics
   with `libnode.dll not found in any search path`. Electron is not that shared
   Node runtime. No fake DLL/import library or dependency-source edit was used.
2. **cargo-xwin/MSVC ABI:** cargo-xwin 0.23.1, user-local LLVM 19 and the Microsoft
   SDK/CRT downloaded successfully. The build reached vendored OpenSSL 3.6.3.
   Its `VC-WIN64A` configuration refuses the Linux Perl implementation:
   `This perl implementation doesn't produce Windows like paths (with backward
   slash directory separators).` The build exits 101 before linking fb-node.
3. **Existing core portability (independently checked):**
   `crates/fb-core/src/attachments/storage.rs:10` unconditionally imports
   `rustix::fs::{Mode, OFlags, open, openat}` and uses Unix directory handles,
   ownership checks, linkat and unlinkat. Rustix 1.1.6 explicitly excludes its
   `fs` module on Windows. A minimal Windows-target Cargo check with those same
   imports reproduced E0432 (`could not find fs in rustix`). A Windows-native
   build will need a reviewed Windows storage backend even after the OpenSSL
   tooling issue is resolved. Do not disable attachments or weaken its
   no-follow/ownership/atomic-write rules to make an installer compile.

These are native build/core portability issues, not missing administrator
permission. No Windows VM was accessed. No application or installer UI was
tested under Wine; Wine is only an optional installer build tool.

## Ready for the next Windows build

- `builder.cjs`: canonical one-click/per-user NSIS target, no elevation, Start
  menu and desktop shortcuts, uninstaller, stable app ID, preserves user data.
- `windows/FreedomBooks.nsi`: standalone fallback NSIS source; requires the real
  native module before it compiles. It uses the WinShell plug-in from
  electron-builder's NSIS resource bundle for shortcut AppUserModelId.
- `build-native.py win32`: MSVC/static-CRT native build on Windows, attempted
  cargo-xwin route on Linux. The current unported core still blocks it.
- `.github/workflows/release.yml`: Windows-latest build plus artifact checks,
  alongside Linux builds. The Windows build is expected to fail until the core
  portability issue is fixed; the dependent release job cannot upload a partial
  Linux-only release. Project LICENSE is also required at publication.

After the core port, build fb-node on Windows, run `installers.cjs win32`, verify
the final `.exe` with `verify-artifacts.py --release`, then complete both Windows
10/11 checklists in `INSTALL-TESTING.md`: standard-user/no-UAC install, native
module load, DPAPI/password behavior, first run, lock/unlock/restart, both
shortcuts/taskbar identity, offline/no-runtime install, reinstall/upgrade, and
uninstall/reinstall preserving the book. None of those Windows results are
claimed by this job.
