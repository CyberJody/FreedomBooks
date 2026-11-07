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
- Whether it needs access to the computer, the company file, the app lock passphrase, or a plugin account (for example a bank feed or ChatGPT connection).
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
