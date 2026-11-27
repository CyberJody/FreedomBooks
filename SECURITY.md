# Security policy

FreedomBooks keeps people's financial records, so we take security reports seriously. Thank you for taking the time to tell us.

## Supported versions

FreedomBooks is currently published as a source preview. Security fixes target the latest public source. No packaged production release is currently supported.

| Version | Supported |
| --- | --- |
| Latest public source preview | Yes |
| Older snapshots and internal builds | No |
| v1 (the frozen Python reference) | No |

## How to report a problem privately

Please do not open a public issue or discussion for a security problem. Use GitHub's private vulnerability reporting instead:

1. Go to the [Security tab](https://github.com/CyberJody/FreedomBooks/security) of this repository.
2. Click **Report a vulnerability**.
3. Fill in the form and submit it.

Only you and the maintainers can see the report. You need a free GitHub account to send one. We do not take security reports by email or any other channel.

## What to include

- What the problem is and what an attacker could do with it.
- The steps to reproduce it, as short as you can make them.
- The FreedomBooks version and your operating system.
- Whether it needs access to the computer, the company file, the app lock passphrase, a plugin account (for example a bank feed or ChatGPT connection), or a computer on the same network or tailnet as a host that is sharing its books.
- Any proof-of-concept code, logs or screenshots.

Please use a test company file with made-up data. Never send real books, passwords, keys, bank credentials or tokens, even in a private report.

## What to expect

- We will confirm we got your report within 7 days.
- We will tell you whether we can reproduce it and how serious we think it is.
- We will keep you updated while we work on a fix, and tell you before we publish it.
- When the fix ships, we will publish a GitHub security advisory. We will credit you by name or handle if you want, or keep you anonymous.

Please give us a reasonable time to fix the problem before you talk about it in public. If you think we are taking too long, say so in the report thread.

## Scope

In scope: the code in this repository, including the Rust core, the desktop app and the plugins, and the release files published on GitHub Releases.

Out of scope: problems in third-party services the plugins connect to (banks, bank feed providers, ChatGPT) unless FreedomBooks makes them worse, and attacks that need full control of an already unlocked computer.

## Desktop book protection

The built-in key is public: Anyone who gets a copy of this file can open it.
Own-password books and independent connection credentials have different threat
boundaries. See [the current key threat model](docs/KEYS-THREAT-MODEL.md) and
[the key architecture](ARCHITECTURE.md).

## Local AI connections

When an Administrator connects an AI assistant, locked access starts ticked by default (they may
untick it, or switch it off in Manage AI connections) to authorize a remembered connection to act on
their behalf while the books are open and the desktop is locked. Each call must match authorization in the encrypted book,
a proof sealed by the operating system and a revocable anchor in the local app profile. An account
or two-factor change replaces the book's record with a random value, or removes it, whatever the
AI connections folder holds, so no file in that folder can turn locked access back on after it. AI traffic
stays on loopback and uses the existing book lease. The interactive session still requires sign-in.
Any program running as the same OS user with the keyring unlocked can use the connection as the
approver within its approved company and permissions. With locked access enabled, the FreedomBooks
lock does not stop that access. After a reboot, the desktop session and keyring must be usable,
FreedomBooks must be running, and the original approved books must be open. This does not bypass
the books' own protection.

Forget replaces the local approval epoch even if book cleanup fails. With the approved books open
it removes the verifier first, whatever the AI connections folder contains or allows, and restoring
profile files alone then cannot bring back the locked-access approval. The approver's own switch
and an account or two-factor change remove or replace the verifier in the same way; none of them
can be stopped from the AI connections folder. An anchor that can be neither removed nor replaced
is emptied in place when the file itself can be written; only a small regular file with no other
name is ever emptied. Forget reports a file it had to leave.
Closed books, other open books, restored previews and a failed write to the book can leave the
verifier in place. A program running as the same OS user can cause that failure by holding the
book's write lock while Forget runs, or while locked access is being turned off. Forget then warns
that book cleanup did not finish, and the approval can come back with its files. Quitting
FreedomBooks stops the access; after starting it again, Forget should be chosen straight away. Forget is a
dependable cut-off only when it ends without that warning. Sign-in attempts to remove verifiers
whose anchor is missing, empty or not a regular file.
Restoring the matching files before a successful missing-anchor cleanup can leave the restored
approval usable even after the next sign-in. Restoring matching older encrypted book contents in
place and matching profile files can also restore consent. Neither case requires rolling back the
OS key store. A book restored as a new file identity cannot inherit that approval.
[AI assistant connections](docs/local-ai.md)
