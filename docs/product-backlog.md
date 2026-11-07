# Product backlog

## Owner feedback — November 7, 2026

Checked items are implemented on the development branch. Internal build verification and broader release readiness are tracked separately.

- [ ] **Named users and QuickBooks-like sign-in.** First support named people on the same desktop, show who is signed in, and allow switching users. Accounting actions must retain the actual user's identity in the audit history. Simultaneous access from other computers is later work.
- [ ] **AI access to accounting workflows.** Let an authorized AI read reports, create invoices, categorize bank activity and import data through the same validated accounting operations as the desktop. Provide a local API and MCP access with explicit permissions, revocable access and actor audit.
- [ ] **Faster transaction review.** Add keyboard navigation, bulk categorization and easier reusable rules to the daily bank-review workflow.
- [x] **Direct transaction review navigation.** Put Review transactions first under Banking, with a distinct active-page indicator.
- [x] **Copy recovery key.** Add an explicit Copy button when the encryption/recovery key is generated, with clear confirmation that it was copied, so it can be saved without manually selecting the text.
- [x] **Right-click editing.** Provide the usual Cut, Copy and Paste menu in editable fields, particularly the SimpleFIN setup-token field, alongside keyboard shortcuts.
- [x] **Create an account while mapping SimpleFIN.** Offer Add new account directly in the mapping screen. Suggest the bank-provided account name and let the user edit it. Offer Checking, Savings and Credit card as account types. Create the account with the appropriate accounting type and select it for the mapping without losing other mapping choices.

- [x] **Explain SimpleFIN app setup.** Direct users to My Account → Apps → New app connection, create a separate FreedomBooks connection and copy its setup token. Keep existing bank connections and other apps.
