# FreedomBooks

FreedomBooks is a desktop accounting application for small businesses, built with Rust, SQLCipher and Electron. Your books stay on your computer. Bank downloads use your own SimpleFIN Bridge account.

## Source preview — November 7, 2026

This is a **code-only preview** of the version being used and tested internally on Linux. It includes the working SimpleFIN connection and transaction-review workflow. There are no installer downloads in this publication, and it is not a production-readiness certification. Use a synthetic company when evaluating it; broader compatibility and recovery verification remains in progress.

The application source comes from the internally tested `a9a008652c39525a02cd3a0a85a68865840b86be` snapshot. The public repository starts with a source snapshot; private development history, local books, credentials and build artifacts are not published.

## Available in the desktop app

- Company creation with starter accounts, editable business details and document numbering.
- Customers, vendors, items, estimates, invoices, receipts, bills and payments.
- Checks, deposits, transfers, bank registers and reconciliation.
- Financial reports, audit history, document PDFs and attachments.
- Book protection, recovery keys, portable backups and restore.
- SimpleFIN bank downloads, account mapping with inline account creation, transaction review and rules. **Review transactions** is the first item under Banking.
- Copying recovery keys and native right-click Cut, Copy and Paste menus.

Bank access is read-only. Downloaded activity stays in review until you accept it into the books. In SimpleFIN Bridge, use **My Account → Apps → New app connection** to create a separate connection for FreedomBooks; keep your existing bank connections and other apps.

Named desktop users, broader AI access and transaction-review improvements are the next development work. They are not included in this snapshot. The existing optional receipt assistant and email plugins are separate from the SimpleFIN-enabled build below. Payroll is not supported. See the [product backlog](docs/product-backlog.md).

## Build and run on Linux

Prerequisites: stable Rust, a C/C++ toolchain, Perl, pkg-config, and Node.js 22.12 or newer. SQLCipher and OpenSSL are built from source. Electron also needs the usual Linux graphical libraries. A working desktop secret store is required to save bank connections.

From the repository root:

```sh
npm ci --prefix electron
FB_SIMPLEFIN=1 npm run build:native --prefix electron
npm test --prefix electron
npm start --prefix electron
```

`FB_SIMPLEFIN=1` builds both the native application bridge and the separate network helper. Set it again whenever rebuilding this configuration. The default build without this flag hides bank feeds. The broader experimental plugin build uses `FB_PLUGINS=1` instead.

For isolated evaluation, set `FB_DESKTOP_USER_DATA` to a new absolute directory before starting Electron. Never point development or test commands at your real books. The normal Linux profile is under `~/.config/app.freedombooks.FreedomBooks`.

Choose your own password for protection. **The built-in key is public: anyone with a copy of that book file can open it.** Saving a bank connection requires your own book password, asking for it when opening, and OS-backed secret storage. See [Book protection](docs/BOOK-PROTECTION.md).

## Development and checks

```sh
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
npm test --prefix electron
```

Start with tests for the behavior you change; the full workspace includes long-running fuzz and compatibility tests. Historical Django differential and screen-comparison checks need a separate frozen reference environment. See [Contributing](CONTRIBUTING.md#running-the-tests) and [development checks](docs/development-process.md).

The code is organized under `crates/` (accounting core, database, app, templates, bridges and plugins), `electron/` (desktop shell) and `packaging/` (packaging tooling). Packaging tooling is retained as source, but packaged releases are outside this preview.

## Contributions and support

Contributions use the [Developer Certificate of Origin](DCO), with a sign-off on contributed commits; no CLA is required. See [CONTRIBUTING.md](CONTRIBUTING.md).

- [Report a bug or request a feature](https://github.com/CyberJody/FreedomBooks/issues/new/choose).
- [Ask a question](https://github.com/CyberJody/FreedomBooks/discussions).
- Report security problems privately using [SECURITY.md](SECURITY.md).

Keep financial data, credentials and book files out of reports and screenshots.

## License

FreedomBooks is licensed under the [GNU Affero General Public License, version 3 or later](LICENSE), with the [Electron/Chromium additional permission](LICENSE-EXCEPTION). Dependencies retain their own licenses; see [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
