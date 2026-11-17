# Sales-tax setup

A company without a saved sales-tax decision sees the question on Home and in
Finish setting up. Choose Yes and select an existing active rate/group, or enter
a name, combined percentage and tax authority. Choose No to save an explicit
no-tax decision without creating an authority or rate. Finish later leaves the
question unanswered.

The authority, new rate and company default are saved in one Writer transaction.
A stale form, duplicate name, invalid rate, ineligible default or failed settings
write saves nothing. An unambiguous active payee with the same authority name
is reused. An inactive or ambiguous match requires review. Payees remain linked
internally so remittances can identify the recipient; no bill or payment is made.
Existing tax rates can also be maintained through Sales tax → Tax rates & groups.

New sales documents start with the company default. Customer exemptions and item
Taxable flags still apply. Existing saved documents retain their stored rates and
amounts. This setup does not change the report basis or filing frequency; review
those separately under Sales tax → Settings. Initial defaults remain invoice-date
(accrual) and monthly reporting until configured there.

`app_TaxSettings` exposes the saved decision, default, reporting settings and
revision through the company-scoped read-only AI API. The guided form continues
to use a path-bound, single-use SubmitForm token; the renderer gets no database
or unrestricted command access.

Design reference: SlowBooks' company default-rate setting in
[settings.js](https://github.com/VonHoltenCodes/SlowBooks-Pro-2026/blob/main/app/static/js/settings.js).
Its source was inspected for workflow reference; this implementation retains
FreedomBooks' exact arithmetic, tax groups, audit and saved-document snapshots.
No SlowBooks source code was copied.

Synthetic regression coverage: d3_flow (atomic rollback, stale/replayed forms,
authority reuse, explicit No, cross-company rate rejection and invoice defaults),
ai_api (read-only settings coverage), and smoke-sales-tax-setup.cjs (real Electron
initial setup, invalid retry, invoice tax/discount totals, saving and reopening).
