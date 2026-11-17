# CyberBrain invoice migration

The user approved bringing CyberBrain's invoice and email appearance, Zelle and
Stripe payment settings, Google OAuth, Gmail Drafts/send choice, and Stripe payment
status updates into FreedomBooks. Stripe uses desktop refresh and catch-up on
reopen. No hosted webhook receiver or hosting service is part of this version.

## Implemented foundation

- Company-specific, revision-checked invoice preferences saved atomically with
  company details and recorded in actor audit. Providers start disabled, Stripe
  defaults to test mode, and the email action defaults to Gmail Drafts.
- CyberBrain-style invoice printing, including company branding, four-column
  items, Zelle instructions, totals, and a configurable footer.
- A compact branded HTML email preview using the same invoice and live balance.
  Previewing does not create a draft or send a message.
- Payment instructions only appear for the current posted invoice with a positive
  balance. Draft, void, paid, and earlier revisions do not solicit payment.
- Additive schema migration 0188, with an authenticated pre-migration fixture,
  verified-backup/upgrade/reopen regression, and retained historical migrations.

The source now includes opt-in Google delivery (`FB_INVOICING=1`), with a
company-specific Desktop OAuth registration, PKCE in the system browser, an
OS-sealed credential envelope, and network calls confined to `fb-net`. The default
remains Save to Gmail Drafts. Sending is an explicit action. Both use the same
frozen invoice/email and attach a main-process-generated PDF and embedded logo.

Delivery history distinguishes preparation, submission, saved draft, accepted send,
refusal and uncertainty. It records acceptance before refreshed-token persistence.
Unknown outcomes require an explicit Gmail check before a fresh attempt; abandoned
attempts can be closed by their actor or a company administrator without resending.
An invoice, balance, company setting, connection or user change invalidates an
outstanding preparation. Migration 0189 is additive and preserves the actual 0188
book schema and data. No live Google account has been connected or emailed during
development. This is not an installed desktop release.

## Stripe payment links and desktop catch-up

The same opt-in build now includes separate company-scoped TEST and LIVE Stripe
connections. Credentials stay in the OS-sealed vault, and all HTTPS is confined to
`fb-net`. A restricted key needs account read, Payment Links write, Checkout
Sessions write, and PaymentIntents/Charges read access. This first version supports
USD invoice balances from $0.50 through $999,999.99. TEST links never appear on
customer invoices or emails, and TEST payments cannot post to the ledger.

Prepare a payment link from a current posted invoice. FreedomBooks creates one
Stripe Payment Link for that exact balance, limited to one completed checkout,
and reuses its saved identity. A durable request and provider idempotency key
prevent blind retries after an uncertain response. Unknown requests must be
recovered or explicitly reconciled with Stripe before another link is prepared.
Before a new request, the app checks Stripe for invoice links missing from a
restored local snapshot. Active known links that were closed locally become
blocking again. Unknown remote links require reconciliation; the app does not
adopt them or guess which restored accounting history is authoritative.

While the company is open, the desktop checks every 30 seconds for one page of
work, refreshing each known link about every 15 minutes as capacity permits.
Never-checked and least-recently-checked links go first. Saved cursors resume
interrupted catch-up, and manual refresh is available on the Stripe payments
screen. No background service runs while FreedomBooks is closed. Old closed links
remain eligible for refresh so later refunds or disputes can be surfaced.

Downloading a successful payment does not mark the accounting invoice paid.
Review closes the link and expires open checkouts, then downloads the full session
history before allowing the invoice balance to change. Posting performs another
fresh check and uses the existing balanced, atomic receipt workflow: gross amount
to Undeposited Funds against Accounts Receivable. Provider identities, invoice
revision, exact money, company, currency, mode and reviewed evidence must match.
Duplicate sessions or payment intents cannot create a second receipt. Stripe fees
and payouts are separate accounting work. Pending settlements, refunds, disputes,
partial amounts and mismatches stay explicit; they never automatically post or
reverse accounting entries.

Recovery, creation preflight and full pre-post scans are bounded at 2,000 provider
links or sessions per operation. Larger histories fail closed and need manual
reconciliation; the app does not treat a partial scan as proof that a payment or
link is absent. Balance-changing actions remain guarded while a live link or
unreviewed payment is unresolved. TEST activity does not block real receipts.

Migration 0190 is additive, with a fixture authenticated against the actual
committed 0189 migrations and a verified-backup/upgrade/reopen regression.
Historical migrations remain unchanged.

## Remaining acceptance gates

The source integration has synthetic core, app, helper-process and desktop
verification plus independent source review. No live Google or Stripe account was
connected, no customer email was sent, and no real payment was created during
development. Test-account provider acceptance and packaged desktop checks remain
required before an internal desktop update. The installed app is unchanged.

Historical invoice import and a loyalty calculation engine are outside this first
version. Work tickets and billable time now exist as job tickets; see
[job tickets](job-tickets.md).

## Google setup and verification

Use a Google Cloud OAuth client of type Desktop app with the Gmail API enabled.
FreedomBooks requests `gmail.compose` for both drafts and send, and stores the
registration/token only in the company-scoped OS-sealed vault. Google consent,
testing-user limits and restricted-scope requirements still apply. A saved draft
is not a sent email, and Gmail acceptance is not proof of recipient delivery.

References: [Desktop OAuth](https://developers.google.com/identity/protocols/oauth2/native-app),
[Gmail scopes](https://developers.google.com/workspace/gmail/api/auth/scopes),
[drafts](https://developers.google.com/workspace/gmail/api/guides/drafts).

Synthetic verification includes real helper-process OAuth/PKCE, MIME/PDF/logo,
refresh/revocation and ambiguous responses; core/app persistence, company and
actor boundaries, duplicate refusal, stale settings and admin interruption
recovery; authentic prior-book backup/upgrade/reopen; and real Electron hidden
PDF rendering with strict network/script isolation and multi-page coordinates.
Live provider acceptance and the packaged internal desktop remain separate gates.

Stripe synthetic verification covers separate TEST/LIVE access, exact invoice
links, pagination/catch-up, delayed settlement, refunds/disputes, duplicate
rejection, stale reviews, closing open checkouts, offline and ambiguous outcomes,
reactivated links, and missing remote history after restoration. The real helper
executable is exercised against a loopback fake; production has a fixed Stripe
HTTPS host and no loopback override. Core, app and native-module dependency trees
are checked to exclude HTTP/TLS clients.

`electron/scripts/smoke-invoicing.cjs` uses a separate Xvfb display, fresh profile,
and synthetic encrypted book. It checks the default modes, real settings form,
multiline persistence, unavailable-keyring refusal, native worker session binding,
and close/reopen behavior. `smoke-invoice-email.cjs` separately exercises real
Electron PDF rendering and its main-only capability. Neither smoke connects a
provider or uses an installed company file.
