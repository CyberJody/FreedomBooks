# Desktop signing plan

Use Node 22.12 or later (Node 24 was verified). The local packaging step is
`npm ci --prefix packaging`, then `npm run build:native`
in `electron`, then `npm run package --prefix packaging`. It builds an ASAR, unpacks
only the native module, applies the five security fuses and reads them back from
the resulting executable. Signing happens after those steps. Never sign first
and then modify an executable or its ASAR.

Windows: obtain an Authenticode code-signing identity using a hardware-backed key
or hosted signing service. Sign the executable, native DLL and installer with
SHA-256 and an RFC 3161 timestamp, then verify the chain and signatures on a clean
Windows machine. Keep the certificate and service credentials outside the repo.

Linux: sign the Flatpak repository summary and commits with the release GPG key;
ship a signed bundle with the public verification key. Sign the AppImage with
its supported embedded GPG signature, and publish a detached signature of the
SHA-256 checksum file alongside release artifacts. Verify both on a clean install.
Keep the release private key offline or in the release runner's secret store.

macOS comes later, when a Mac build runner is available: Developer ID signing,
hardened runtime with minimal entitlements, notarization and stapling. Package,
set fuses, sign, notarize, then verify Gatekeeper on a clean Mac.

Unsigned alpha packages must be described as unsigned. No certificate purchase,
credential setup, remote build or publication is part of SEC-FIX1.

## Installer release workflow (local preparation only)

`.github/workflows/release.yml` builds on a `v*` tag and attaches unsigned alpha
artifacts only after the build and verification jobs pass. This workflow is
committed for review; do not push/tag/run it before publication is authorized.
The repository's project LICENSE (including the approved Electron additional
permission) is required by release CI. Local preparation permits its absence.

Reserved GitHub Actions secret names, read only by the signing placeholder:

- `FB_WINDOWS_SIGNING_CERT_BASE64`: future exported certificate input.
- `FB_WINDOWS_SIGNING_PASSWORD`: future certificate password.
- `FB_LINUX_GPG_PRIVATE_KEY`: future Linux release signing key.
- `FB_LINUX_GPG_PASSPHRASE`: future Linux key passphrase.

Keep these unset for the unsigned alpha. `packaging/signing-placeholder.py` fails if signing
credentials are configured: the placeholder must be replaced with a verified
signing implementation before using them. It never prints their values.
Hardware-backed/hosted Windows signing may replace the certificate input with
service-specific secrets in a later reviewed change. No credentials are acquired
or certificates created here.

Order for that future implementation: build the native module; pack the ASAR;
edit PE resources/embedded ASAR integrity; set and read back fuses; sign the native
module and application executable; wrap with NSIS; sign the installer; extract
and reverify fuses, ASAR integrity, notices and signatures; generate checksums;
attach artifacts. Signing may change executable bytes, so update the verifier's
unsigned input-byte comparison explicitly when signing is implemented. Never
silently disable its comparison.

Fuse setup follows [Electron's documentation](https://www.electronjs.org/docs/latest/tutorial/fuses)
and [the official fuses package](https://github.com/electron/fuses). ASAR integrity
enforcement is supported by Electron on Windows/macOS; the fuse is also set and
verified in Linux builds, whose release signatures provide artifact verification.
