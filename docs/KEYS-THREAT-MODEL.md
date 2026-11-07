# Book keys: threats and limits

The default built-in key provides convenience and portability, not confidentiality.
Anyone who gets a copy of this file can open it. The published key and SQLCipher
format are public. A raw-byte scan finding no readable company text is not a
security proof for this mode; deliberately opening with the public key is the
positive control. Do not market these books or their backups as encrypted.

Own-password mode protects copied books/backups against an attacker without the
password, recovery key or an authorized OS-sealed master. Password strength still
matters: each guess against a copied record or backup costs one Argon2id run at
64 MiB, three passes (the same as the earlier app-lock wrap). The recovery key is an alternative credential with 150 random bits.
Keep printed/exported Emergency Kits separate; these explicit user exports contain
the key in plaintext and are private files. They are excluded from leak scans only
as named test artifacts, not by broad text/path patterns. “Saved” means a completed
save or accepted print operation, not proof the user retained the paper.

Auto-open deliberately trusts the logged-in OS user and keyring. A process running
as that user, a compromised renderer during authorized recovery display, an
unlocked application, malware, root/admin and live-memory capture are outside the
copied-file guarantee. V8 strings passed to safeStorage/export cannot be reliably
zeroized. The renderer sandbox, CSP, no-network rule and narrow IPC remain required.

Connection secrets use a separate random installation key. Possession of the
built-in key, own-password master, password or recovery key alone cannot decrypt
new credential envelopes. SimpleFIN connect and sync require usable OS sealing;
without it, the setup token is not claimed and saved credentials are not sent.
Historical profiles may include a 0600 fallback key; filesystem permissions do
not protect that key from its owner or an administrator. Those files and saved
connections are retained, and can be sealed when a keyring becomes available.
Restoring on another installation requires reconnecting.
Older book-derived credentials are migrated on first legacy unlock; pre-migration
copies retain the old exposure. Add-password cannot revoke old built-in backups.

R11 still requires known connection secrets and their normalized forms to stay
out of rows, audit details, exports, logs, error messages and backup side files.
Its raw-file unreadability assertion now applies only to own-password books.
For built-in books, the replacement proof opens with the public key and shows that
credential envelopes cannot decrypt under that key; restored connections require
reconnect. Masks must still install before any mutation. No production secret,
real keyring, real home/profile or live book is used by the tests.

Crash tolerance covers process death and filesystem durability ordering. It does
not promise survival of hardware failure or corruption of both the live book and
verified rollback snapshot. A failed snapshot hash refuses automatic rollback and
directs the owner to the verified backup. Backups on separate media remain needed.
