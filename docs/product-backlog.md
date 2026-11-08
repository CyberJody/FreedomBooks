# Product backlog

## Owner feedback — November 8, 2026

Checked items are implemented on the development branch. Internal build verification and broader release readiness are tracked separately.

- [x] **Named users and QuickBooks-like sign-in.** Named people on the same desktop, username/password sign-in, roles and company permissions, visible signed-in identity and switching users. Accounting actions retain the actual user's identity in the audit history. See [Desktop users](desktop-users.md).
- [x] **AI access to accounting workflows.** Authorized AI can read reports, create invoices, categorize bank activity and import structured journal batches through validated accounting operations. Local API and MCP access have explicit permissions, revocation and actor audit. See [supported operations and limits](local-ai.md).
- [x] **Faster transaction review.** Keyboard selection, previewed bulk draft edits and reusable rules are available. Applying bulk edits does not post transactions.
- [x] **Authenticator-app MFA.** Optional per-user MFA with local QR setup, manual fallback and single-use recovery codes.
- [ ] **Simultaneous access from other computers.** The current version supports one signed-in person at a time on one desktop.
- [ ] **YubiKey sign-in.** Deferred; authenticator-app MFA is available now.
- [x] **Direct transaction review navigation.** Put Review transactions first under Banking, with a distinct active-page indicator.
- [x] **Copy recovery key.** Add an explicit Copy button when the encryption/recovery key is generated, with clear confirmation that it was copied, so it can be saved without manually selecting the text.
- [x] **Right-click editing.** Provide the usual Cut, Copy and Paste menu in editable fields, particularly the SimpleFIN setup-token field, alongside keyboard shortcuts.
- [x] **Create an account while mapping SimpleFIN.** Offer Add new account directly in the mapping screen. Suggest the bank-provided account name and let the user edit it. Offer Checking, Savings and Credit card as account types. Create the account with the appropriate accounting type and select it for the mapping without losing other mapping choices.

- [x] **Explain SimpleFIN app setup.** Direct users to My Account → Apps → New app connection, create a separate FreedomBooks connection and copy its setup token. Keep existing bank connections and other apps.
