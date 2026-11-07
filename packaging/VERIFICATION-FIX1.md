# Installer fix 1 verification — 2026-11-02

Code verified through `cf3aebf`, continuing `pkg-installers` from `1869e9e`.
Local unsigned artifacts only; no merge, push, publication, signing or real sign-in.
The original verification remains in `VERIFICATION.md` as historical evidence.

## Findings and fixes

| Finding | Fix | Verification |
|---|---|---|
| P1: native module missed beneath hidden CI TMPDIR | `package.cjs:49` uses ASAR's basename matching (`*.node`). | `asar.test.cjs:8` creates a native module below `.work/tmp`; actual native build, packager and both installer formats pass with the workflow TMPDIR. |
| P2: books and keys under configuration | `electron/main/identity.cjs:20` selects XDG data/FreedomBooks. `migrate-data.cjs:14` stages/resumes moves, uses rename or copy/fsync/SHA256/remove, preserves 0600/0700, and refuses conflicting destinations. Settings retain the existing Electron profile. | Five migration tests, three identity tests, and actual old `.deb` → fixed `.deb` upgrades on Debian and Mint; before-unlock hash checks and successful password unlock. |
| P2: unconditional setuid helper | `linux/after-install.sh:8` starts at 0755, reads kernel policy directly from procfs, loads a binary-specific AppArmor userns profile where required, and logs the reason for any 4755 fallback. `after-remove.sh:3` unloads/removes the profile on remove/purge. | Seven fake-sysctl policy cases; real Mint AppArmor syntax check; Debian/Mint ordinary-user smokes at 0755 with renderer Seccomp=2 and NoNewPrivs=1. |
| P3: document-portal cleanup failure | `test-appimage.py:36` calls `cleanup-home.py:37`, detaching only mounts inside this run's HOME or safely retaining the mount and its ancestors. | Mount-pruning regression preserves the mounted document; final AppImage direct and extract-and-run tests each pass 6/6 and exit 0. |

Electron unit tests: **14 passed**. ASAR hidden-path regression: **1 passed**.
Sandbox policy: **7 passed**. Portal cleanup regression: **1 passed**.
The initial preliminary unpacked smoke timed out during schema creation under
heavy disk writeback; the next run passed 6/6. The smoke driver now captures a
failure page and allows five minutes for encrypted schema initialization, with
the same assertions. All final artifact smokes passed.

## Actual package lifecycles

The independent review's `lifecycle.py`, `reopen.cjs`, and `mint-container.sh`
were reused and extended in the evidence directory. The old `.deb` is checked
against the `1869e9e` recorded SHA256
`f127f1c54177acf01bcc17a478e0750bd145188a146bde15556e73f6b4bd07d2`.
Its first launch creates the synthetic company and password-wrapped key.

Both Debian 13 and the Mint 22 build container (Noble/Wilma repositories) passed:

- old-version first-run/restart smoke: 6/6;
- remove/reinstall and unlock;
- actual d1 → d2 upgrade: old paths disappear on first launch; the new book/key
  hashes match before unlock; the same company unlocks;
- purge/reinstall and unlock, followed by fixed-version remove/reinstall/unlock;
- final fixed-version first-run/restart smoke: 6/6, helper 0755;
- all recorded book/key SHA256 values identical throughout the lifecycle;
- files 0600, data/books directories 0700, settings profile retained;
- program files root-owned, no world-writable or setuid/setgid files.

The separate fresh Debian `.deb` dependency/install/smoke/uninstall test also
passed 6/6 and exited 0. Product dependencies were resolved before test tools.
Applications ran as ordinary users (Debian UID 1000, Mint UID 1001); apt used root.

## CI paths, artifact integrity and publication boundary

Node 24.22.2, Python 3.13.5 and installed Rust stable 1.99.1 were used. Locked
packaging/driver dependencies were installed with the workflow's `npm ci`
commands; `build-native.py linux`, `installers.cjs linux`, artifact verification,
both Linux smoke drivers and the unsigned signing placeholder ran locally.
The final maintainer-script-only adjustment was rewrapped through the canonical
`installers.cjs linux --wrap-only` path and every artifact gate was repeated.
Both builds used the workflow's exact TMPDIR:

```text
/mnt/Data/Projects/FreedomBooks/wt-v2/pkg-installers/packaging/.work/tmp
```

All five fuses read back correctly on the hardened input and final extracted
artifacts. The reused independent audit checked all **118 ASAR file and block
hashes**. Linux runtime ASAR integrity enforcement is not claimed. Electron's
LICENSE and LICENSES.chromium.html are byte-identical to upstream in both
artifacts; THIRD-PARTY-NOTICES is identical to source. Exact ASAR production
input has zero third-party npm production dependencies.

The exact `python3 packaging/ci.py linux` entrypoint was attempted and **exits 1**:

```text
Release blocked: add the approved project LICENSE at publication
```

This intentional, pre-existing publication guard was preserved. The brief and
AGENTS forbid adding a project LICENSE. Therefore the complete release driver
cannot honestly be reported as passing; its Linux build/test components passed.
No GitHub Actions run, runner provisioning, upload, signing or release occurred.

| Final artifact | Bytes | SHA256 |
|---|---:|---|
| `FreedomBooks-0.1.0-d2-amd64.deb` | 108,709,708 | `b2383fc6d634b28c5b6a847a6d959dc46aa527d3b7ce32f8508d8834ddd3180d` |
| `FreedomBooks-0.1.0-d2-x86_64.AppImage` | 126,020,088 | `6380fc117b40a1b4caa7b6ef99aa968a077d5205f481c2c1c47fd8dc9a1b9bd3` |

Artifacts and checksum files are local ignored outputs in `packaging/dist`.

## Full project gate

`scripts/gate.sh` exited **0**: **1,489 passed test executions, 0 failed,
2 ignored**. The workspace contributed 1,379 passes and the additional JSON
feature-variant run contributed 110. These count executions across variants,
not unique test names.

```text
SCREENS MATCH — 512 screens, 0 with differences
gate: clippy, check_json_features.sh (workspace tests + JSON feature unification), core isolation and screen diff passed
```

This job's two isolated PostgreSQL verification databases were dropped after
the gate completed. No shared/default verification database was used.

## Limits and evidence

The Mint build image identifies as Ubuntu 24.04.5 and uses the Mint Wilma
repositories. The host kernel has no
`kernel.apparmor_restrict_unprivileged_userns` interface.
Containers share that kernel and lack AppArmor securityfs. Mint's real parser
accepted the shipped profile with `-Q -K`; `apparmor_parser -r` failed because
the AppArmor filesystem/interface was unavailable. Actual Ubuntu/Mint kernel
enforcement could not be exercised. The fake-sysctl tests exercise successful
profile loading/removal and parser-failure/setuid fallback, without changing
host policy. Real Mint desktop kernel enforcement remains a machine check.

The `pkg-flatpak2` manifest was inspected: it already confines XDG_DATA_HOME to
the application sandbox, requiring no additional filesystem grant. Current
packaged code ignores its old development-only FB_DESKTOP_USER_DATA export and
chooses data/FreedomBooks. Preserve that packaged-override guard at integration.
No Flatpak rebuild or Windows execution was performed for this Linux fix job.

Evidence: `/mnt/Data/Projects/FreedomBooks/staging/pkg-installers-fix1/`, including
build/rewrap, Debian/Mint, AppImage direct/extracted and gate logs; reused scripts;
per-stage lifecycle hashes, mode inventories, and independent ASAR audit. The
first artifact pass is archived under `first-pass/`.

The final Debian/Mint post-upgrade Home and AppImage post-restart Home screenshots
were visually inspected. Screenshots are in `lifecycle-results/`, `mint-results/`
and the worktree's `packaging/.work/appimage-smoke/`. No HTTP preview is involved.

Suggested lessons: test hidden CI paths, prior-version book/key lifecycle hashes,
XDG placement and interrupted/cross-filesystem migration, actual sandbox mode,
AppArmor decision branches, and test-owned FUSE/portal cleanup before trusting a
successful smoke count.
