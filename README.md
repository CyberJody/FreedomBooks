# FreedomBooks

FreedomBooks is a desktop accounting application for small businesses, built with Rust, SQLCipher and Electron. Your books stay on your computer. Bank downloads use your own SimpleFIN Bridge account.

## Source preview — November 27, 2026

This is a **code-only preview** of the version being used and tested internally on Linux and Apple Silicon macOS. New since the November 26 preview: A/R and A/P Aging summaries, Vendor 1099 Summary, NY Sales Tax non-calendar quarters with jurisdiction and taxable-vs-exempt breakdown, fiscal-year start month and inclusive closing-date period locks, customer default payment terms and taxability, per-account bank-feed import start dates and account-scoped rules, credit-card transfers, scheduled automatic backups with verified retention pruning and staged restore, Customer Statements, Direct-Method Cash-Flow Statement, Multi-Period Comparative Profit & Loss, Sales by Customer/Item, Schedule C Activity Worksheet, and Fixed-Asset Depreciation Schedule (all with desktop and connected-host printing and CSV export), plus post-review hardening across cash-basis apportionment, bank-feed staging, cross-platform backup/restore locking, desktop attachment downloads, remote session isolation, locked native packaging, and dependency security updates (`rusqlite` / SQLCipher 4.20.0, `lettre` 0.11.24, `url` 2.5.9). There are no installer downloads in this publication, and it is not a production-readiness certification. Use a synthetic company when evaluating it; broader compatibility and recovery verification remains in progress.

The application source comes from the internally tested `8ae4a09790595d32a824fc55ab4b473cd95e307f` build; application behavior is unchanged, and public documentation and internal test-file notes were adjusted for publication. The public repository contains source snapshots; private development history, local books, credentials and build artifacts are not published.

## Available in the desktop app

- Company creation with starter accounts, editable business details and document numbering.
- Customers, vendors, items, estimates, invoices, receipts, bills and payments.
- Checks, deposits with fee/adjustment lines, transfers, bank registers and reconciliation.
- Financial reports, audit history, document PDFs, client-side printing and attachments.
- Book protection, recovery keys, portable backups and restore.
- SimpleFIN bank downloads, account mapping with inline account creation, transaction review with split rows, late-clearing check matching, multi-entry matching and reusable rules. **Review transactions** is the first item under Banking.
- Copying recovery keys and native right-click Cut, Copy and Paste menus.
- Named users on one desktop, username/password sign-in, roles, company permissions and audit attribution. See [Desktop users](docs/desktop-users.md).
- Optional authenticator-app MFA: scan a locally generated QR code, confirm a six-digit code and save single-use recovery codes. Manual setup is available as a fallback.
- Authorized AI assistant connections through a local API and MCP, including optional locked access for administrators, reports, invoices, bank categorization and structured journal imports. The model can run locally or in the cloud. Connections use the signed-in person's permissions and validated accounting operations. See [AI assistant connections](docs/local-ai.md).
- Keyboard selection, previewed bulk draft edits and reusable rules for transaction review. Applying bulk edits does not post transactions.
- An Expenses page for recording and finding spending in one place.
- Monthly and yearly recurring bills from a template, with skip and catch-up review. See [Recurring bills](docs/recurring-bills.md).
- Job tickets for customer work, with billable time entries that become an ordinary draft invoice. See [Job tickets](docs/job-tickets.md).
- Several people signed in to the same books at once, each with their own user name, password and role.
- Hosting: one computer keeps the books and other computers on your own network or Tailscale use them through it, paired with a one-time code and mutually pinned certificates, with live presence and safety controls. Hosting is off until an Administrator turns it on. See [Host these books](docs/hosting.md) and [Remote access](docs/remote-access.md).

Bank access is read-only. Downloaded activity stays in review until you accept it into the books. In SimpleFIN Bridge, use **My Account → Apps → New app connection** to create a separate connection for FreedomBooks; keep your existing bank connections and other apps.

Hosting never listens on the internet, opens a router port or uses UPnP, and a headless host service is not built yet. Backup, restore and book password or key changes run only at the host computer. YubiKey support is deferred. Local AI access covers the documented operation catalogue, not arbitrary database or file access. The optional receipt assistant and email plugins are separate from the SimpleFIN-enabled build below. Payroll is not supported. See the [product backlog](docs/product-backlog.md).

## Build and run on Linux

Prerequisites: stable Rust, a C/C++ toolchain, Perl, pkg-config, and Node.js 22.12 or newer (`.node-version` pins the tested 24.22.2). SQLCipher and OpenSSL are built from source. Electron also needs the usual Linux graphical libraries. A working desktop secret store is required to save bank connections.

From the repository root:

```sh
npm ci --prefix electron
FB_SIMPLEFIN=1 npm run build:native --prefix electron
npm test --prefix electron
npm start --prefix electron
```

`FB_SIMPLEFIN=1` builds both the native application bridge and the separate network helper. Set it again whenever rebuilding this configuration. The default build without this flag hides bank feeds. The broader experimental plugin build uses `FB_PLUGINS=1` instead. Gmail invoice email and Stripe payment links are a separate opt-in build (`FB_INVOICING=1`); ordinary invoices need no flag. See [invoice email and payments](docs/invoicing-migration.md).

For isolated evaluation, set `FB_DESKTOP_USER_DATA` to a new absolute directory before starting Electron. Never point development or test commands at your real books. The normal Linux profile is under `~/.config/app.freedombooks.FreedomBooks`.

Choose your own book password for file protection. **The built-in key is public: anyone with a copy of that book file can open it.** Individual sign-in and MFA do not change that. The secure OS keyring normally opens protected books in the background; an extra book-password prompt is optional. Keep the book password and recovery key for portable backup restore or an unavailable keyring. Bank credentials require password-protected books and OS-backed secret storage. See [Desktop sign-in and book protection](docs/desktop-users.md#book-unlock-and-individual-sign-in).

## Development and checks

```sh
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
npm test --prefix electron
```

Start with tests for the behavior you change; the full workspace includes long-running fuzz and compatibility tests. See [Contributing](CONTRIBUTING.md#running-the-tests) and [development checks](docs/development-process.md).

The code is organized under `crates/` (accounting core, database, app, templates, bridges and plugins), `electron/` (desktop shell) and `packaging/` (packaging tooling). Packaging tooling is retained as source, but packaged releases are outside this preview.

## Contributions and support

Contributions use the [Developer Certificate of Origin](DCO), with a sign-off on contributed commits; no CLA is required. See [CONTRIBUTING.md](CONTRIBUTING.md).

- [Report a bug or request a feature](https://github.com/CyberJody/FreedomBooks/issues/new/choose).
- [Ask a question](https://github.com/CyberJody/FreedomBooks/discussions).
- Report security problems privately using [SECURITY.md](SECURITY.md).

Keep financial data, credentials and book files out of reports and screenshots.

## License

FreedomBooks is licensed under the [GNU Affero General Public License, version 3 or later](LICENSE), with the [Electron/Chromium additional permission](LICENSE-EXCEPTION). Dependencies retain their own licenses; see [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
