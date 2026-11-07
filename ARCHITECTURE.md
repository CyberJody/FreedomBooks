# Desktop key architecture — KEYS-R1

This tracked design updates the key, backup and first-run sections (§5.2, §5.5,
§6–7) of the project design/ARCHITECTURE.md for Jody’s 2026-11-04 decision.
The accounting schema, core isolation rule and Electron §8 boundary are unchanged.

## One file format; two protection modes

Every book uses SQLCipher. The default uses the published `BUILTIN_KEY` in
`crates/fb-core/src/backups/portable.rs`. Anyone who gets a copy of this file can
open it. There is no plain-file mode, conversion, recovery key or lock screen for
this mode. Own-password books use a random 256-bit master key. Fixed-cost Argon2id
derives independent password/recovery wrapping keys: the owner password uses
65,536 KiB, three passes, one lane (format `freedombooks-recovery-v2`, the cost of
the earlier app-lock wrap); the 150-bit recovery key uses 19,456 KiB, two passes,
one lane (v1). Each format's AAD pins its cost, and v1 password wraps still open.
AES-256-GCM authenticates each master-key envelope. Recovery keys contain
150 random bits, displayed as six groups of five Crockford-base32 characters plus
a seventh check group (25 bits of SHA-256 over the first thirty). Entry ignores
case, spaces and dashes and folds O→0, I/L→1, U→V; a check mismatch is named as a
typing mistake. Keys made before the check group (six groups) still open.
Older app-lock wrappers retain their existing KDF and master on first unlock.

`<book>.keys.json` (0600, atomic rename and directory sync) holds versioned wrapped
keys, a master-sealed copy of recovery text, a master-sealed random receipt key,
and the local prompt/saved-kit flags. Raw own-password master keys and plaintext
recovery text are not serialized. The record parser rejects duplicate/unknown
fields, symlinks, oversized records and unsupported versions. Secret types do
not implement Debug. Recovery display/export requires session authorization;
leaving its screens or locking revokes that authorization.

With “Ask for a password” off, Electron safeStorage seals the master locally in
`<book>.os-key`. The default ask-on setting removes that seal. Linux basic_text
is not a keyring. Without a usable keyring, auto-open is refused. A book with
saved connection credentials cannot disable the prompt or remove its password.
File/idle/suspend lock apply when a password is asked for. No renderer receives
the master key, connection key, or filesystem access.

## Connection credentials have a separate root

`electron/main/keys.cjs` creates a fresh installation secret, sealed by safeStorage
or held in an atomic 0600 fallback file in the private data directory. It never
derives from a book key. fb-app domain-separates that secret with HKDF, then wraps
each credential’s independent data key. The SQLCipher book may carry ciphertext
envelopes, but the public book key cannot unwrap them. Backups never include the
installation secret. A different install marks unreadable bank feeds “Reconnect
needed”; it still restores every available masking capability before writes.
Legacy book-derived envelopes migrate only when opening an older book without a
portable record; the new record prevents that fallback for new portable books.

## Portable backups

Desktop backups with key records use format 4; generic core backups remain format
3 for existing callers. Readers accept both. Format 4 puts the mode and portable
key envelopes in the authenticated header, while manifest, book and receipt
contents retain the existing authenticated archive format. A password OR recovery
key unwraps an own-password backup’s master on another install. Built-in-key
backups need neither and offer no secrecy. The portable record does not copy local
auto-open or saved-kit preferences; protected restores start with the prompt on.
Format-3 archives still require their original master key. Extra-passphrase core
exports remain supported by the core; the desktop keeps its existing refusal.

The independent receipt key preserves immutable per-file key metadata through
SQLCipher rekey. The portable record seals that receipt key under the current
book master, and backup verification uses it to authenticate every receipt.
Previous copies retain access to the data they contained; switching protection
cannot revoke those copies.

## Durable creation and protection changes

First run writes/syncs the final key record, builds the book in a private sibling
`.creating` directory, closes/checkpoints it, syncs it, then hard-links it into its
final name without overwriting. A crash before publication can restart setup;
after publication, the durable key record opens the complete book. An unavailable
key always leaves an editable password/recovery input and restore route.

Changing modes first creates and verifies a complete backup, then makes a keyed
SQLCipher snapshot and a synced rollback journal with the old record and snapshot
hash. SQLCipher `PRAGMA rekey` changes the live file in place, followed by integrity
checking and atomic replacement of the key record. The journal is removed only
after reopening successfully. On startup, any journal causes an idempotent
restore of the verified old snapshot and record before opening. Crash tests kill
the child at first-run boundaries, rekey commit boundaries, and during the actual
PRAGMA page rewrite in both directions. Fault hooks require the explicit
`key-test-hooks` feature, never a normal release build.

SQLCipher rekey behavior: https://www.zetetic.net/sqlcipher/sqlcipher-api/#rekey

## Screen contract and verification

Setup’s checkbox is unchecked; the exact file-copy warning appears in setup,
Settings, Backups and the user guide. The default mode is never described as
encrypted in those surfaces. Settings, Recovery, Emergency Kit and the foreign
backup prompt are deliberate desktop-only screens. Literal wording, mode,
permission and routing assertions live in fb-app/tests/protection.rs and
electron/scripts/smoke-keys.cjs, with 1440×1000 screenshots and PDF/text checks.
The existing Backups screen keeps its narrow comparison rules plus the exact
legacy setup link and “Protection” column heading; changed security text still
must differ. See crates/fb-screendiff/NORMALIZATION.md.
