# Invoice PDF and email presentation

Company settings hold business contact details, the company logo, invoice footer,
Zelle recipient/instructions, and email subject/greeting/signature. The invoice
editor summarizes configured payment options and links directly to those settings.

PDFs and emails share the same exact document values. A draft preview shows the
proposed total and a clearly marked payment-options preview. It never retrieves a
live Stripe URL or enables sending. Paid, voided and earlier invoice versions do
not show payment instructions. A current posted invoice uses the live balance.
Gmail Drafts remains the default delivery action; Send is explicit.

A Stripe QR is generated locally from the same validated current LIVE payment URL
shown in the document. Missing/unprepared links use a labeled placeholder, with
no clickable or scannable substitute. TEST URLs never enter customer documents.
Payment-method marks accompany the checkout section; methods actually available
are determined at Stripe checkout.

Payment artwork and attribution are under renderer/static/sales/pay. Printed
assets are local files. Actual emails reference CID images supplied by the network
helper from a fixed compiled-in allowlist; no external image service or caller-
selected file paths are used. MIME enforces unique safe IDs, eight-image and
128-KB aggregate limits. Package the updated production fb-net together with the
native module; old helpers reject the new brand_images field.

The company-scoped app_InvoiceSettings operation reads non-secret presentation,
logo and connection readiness. command_SaveCompanyDetails uses the existing
administrator-only audited save, including the preferences revision guard.
Neither operation authorizes a provider connection, payment link, email or ledger
posting. Migrating presentation must not copy credentials or source payment links.

Synthetic checks cover exact draft/posted totals, paid/void/earlier suppression,
API permissions and stale writes, local QR output, Gmail CID/limit validation,
strict private-pipe selectors, desktop/small email layouts, one-page short PDFs,
and preservation of long descriptions across multiple pages.
