# Job tickets and billable time

Customers › **Job tickets** records work for a customer and the time spent on it, then turns that
time into an ordinary draft invoice. Tickets and time entries are work records, not accounting
postings: nothing reaches the books until the invoice made from them is posted.

## Tickets

A ticket is numbered `JT-000001`, `JT-000002`, … per company, belongs to one customer for good, and
has a title, description, opened date and optional due date. Its status is **Open**, **Completed**,
**Invoiced** or **Closed without billing**; Completed and Closed record the date. Only open tickets
take time. Completed, invoiced or closed tickets can be reopened. Tickets are never deleted.

The list filters by customer and status (open and completed by default) and shows hours and the
unbilled billable amount. The ticket page shows its time, totals, the invoices made from it, an
**Add time** form and the status actions.

## Time entries

An entry has a date, customer, service item (an item of kind `SERVICE`, such as "On-Site Support"),
hours, rate, a billable flag, notes, who did the work (the person signed in by default) and an
optional ticket of the same customer. Enter time from a ticket, from **Time entries › Enter time**
or the **+ New › Enter time** menu.

- Hours are exact: `3.5`, `3.5h`, `3.5 hrs`, `3:30`, `90m` and `1h 30m` are accepted, up to four
  decimal places and 1,000 hours. Clock time and minutes convert exactly where they can and
  otherwise round half-up to the fourth decimal (1:20 is 1.3333 h), the precision of an invoice
  quantity.
- A blank rate uses the service item's price. A typed rate different from the price is an
  override and is marked on the ticket. Editing an entry keeps its recorded rate (the form shows
  it) unless you type a new rate or choose a different service, so changing notes, hours or date
  never reprices time after the catalog price changes.
- Amount = half-up(hours × rate) to the cent, the same rule the invoice applies to its lines.
- Unbilled time can be edited, moved between that customer's open tickets, taken off a ticket or
  removed. Time that is on an invoice cannot change until that invoice is voided. Time that was
  ever on an invoice is kept with its history (mark it not billable instead of removing it).
- A service item with recorded time stays a service (Items & services refuses changing it to a
  product); create a separate product instead.

## Invoicing a ticket

**Create invoice** on a ticket chooses which unbilled billable entries to bill (all by default),
one invoice line per entry or one line per service and rate (hours added up), the invoice date and
an optional due date (30 days by default). It creates a draft invoice through the invoice service,
with each line's item, income account and taxable default from the service item and customer. Tax
uses the company's default sales tax when it charges tax and a line is taxable; otherwise No tax.
Review, edit and post the invoice as usual. By default the ticket is marked Invoiced once all its
billable time is on invoices.

Each billed entry records which invoice line billed it and the customer it was billed to. An entry
is billed while its line is on the invoice and the invoice's current revision is not void, so the
same time cannot be on two live invoices (the database refuses it). Deleting a draft or voiding a
posted invoice releases its time; the ticket then shows the released time and can be invoiced again.

Editing the draft keeps the time and the invoice in step:

- **Remove a time line** and its entries are released at once (audit `job_ticket.time_released`);
  correct them if needed and invoice them again.
- **Reorder** lines, change a time line's description, tax flag or income account, or add other
  lines freely; billed time follows its line. A copied line is an ordinary charge, not billed time.
- A time line's **service, quantity and rate** cannot change, and the invoice's **customer** cannot
  change while it bills time: the edit is refused with the line named. Remove the time lines (or
  void the invoice) first. The invoice seal in the database refuses the same mismatch.

In "one line per service" mode the line charges half-up of the total hours × rate, which can differ
by a cent from the sum of the entries' own amounts. The line's cents are allocated to its entries
by cumulative half-up rounding, so each entry's invoiced amount is within a cent of its own amount
and they add up to the line exactly; the ticket's **On invoices** total, the invoices list and the
`job_ticket.invoiced` audit amount use these invoiced amounts (the audit also records the time
value as `time_value`). Unbilled time is valued at the entries' own amounts.

## Permissions, audit and the book

Read-only users can view tickets and time; changes need a user who may save changes. Every change
is an append-only audit event (`job_ticket.*`, `time_entry.*`) by the person who made it. Tickets,
entries and billing history are company-scoped and checked again by database guards and the
open-time integrity check. Migration 0205 is book format 92, after recurring bills at format 91.
It adds the four tables and preserves every earlier value guard; it changes no existing row.

AI assistant connections can list tickets (`app_JobTickets`) and, with accounting write, record
time (`command_SaveTimeEntry`); see [AI assistant connections](local-ai.md).
