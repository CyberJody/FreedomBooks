# Recurring bills

Open **Vendors → Bills → Recurring bills** to create a monthly or yearly template. You can also choose **Make recurring** on a bill. That copies its vendor, memo, lines and number of days between bill and due dates. The first date defaults to one period after the source bill and must be strictly after the source date. An earlier billing day is allowed: November 15 to December 1 monthly, or to next November 1 yearly, warns that the dates might duplicate the source period and requires explicit confirmation naming both dates. November 15 to December 15 monthly or next November 15 yearly needs no source-period warning. Supplier references, correction links, attachments and payments stay with the source bill.

A template has a first bill date, terms in calendar days, optional inclusive last date and optional total number of scheduled dates. Skipped dates count toward that limit. Both limits apply when both are set. Monthly schedules keep the original day: January 31 becomes February's last day, then March 31. A yearly February 29 becomes February 28 in other years and returns to February 29 in leap years.

Bills start as drafts for review. Choose **Post automatically** only when the template should record its bills without another posting review. Posting uses the same accounts, numbering, snapshots, permissions and exact arithmetic as an ordinary bill. Drafts do not enter expense reports until posted; posted bills follow the existing accrual and payment-based cash recognition rules.

The desktop checks schedules on company navigation and every 30 seconds while the books are open and an administrator or bookkeeper is signed in. It checks only that operator's companies and does not run in a restored backup, while signed out, or for a read-only user. Audit attributes the operation to that operator and records automatic generation, the template revision, scheduled date and bill number. There is no separate unattended service or fictional system login.

Before saving an active template with any dates due through today, a confirmation shows the number of pending bills and whether they will post automatically. Up to twelve dates are listed; longer lists show the full count and inclusive range, the first six and last six dates, and the number of intervening dates included. This also applies when an edit moves dates earlier. No template changes or bills are saved until confirmation. A compact confirmation digest binds the schedule, revision, cursor, posting choice and full server-computed date list, which are rechecked in the write transaction. A stale date list or changed posting choice must be reviewed again; stale resume choices show the refreshed list rather than retrying the old one.

On **Resume**, missed dates are listed with two choices: **skip** them permanently without creating bills, or **create** their bills. Skips are retained as audited occurrences without a bill, so a restart or concurrent trigger can never generate them later. If there are no missed dates, resume acts directly. Existing templates use Pause/Resume to change active state; editing cannot bypass that choice.

Missed dates for active templates catch up, within the limits below. Each occurrence has a unique template/index and template/date pair and a deterministic normal bill request key. The bill, optional posting, retained occurrence link, number counters and audit commit together. A bill-save failure leaves the occurrence pending and allows a later retry; it does not block other templates. If posting fails validation, its posting work rolls back and the saved draft, occurrence and audit commit with the reason. A message explains why the draft needs review. Dismissed notices stay dismissed for the session. Bill dates always remain their scheduled dates.

Edit a template to change future bill content, terms, posting choice or limits. After its first recorded period, its original start date and frequency stay fixed; create a new template to change that schedule. Changes never rewrite created bills. Pause preserves the next pending occurrence. Delete asks for confirmation, stops further generation and retains the template's history and links from existing bills.

The detail page shows the next twelve scheduled dates and all created bills and skipped periods. Paused dates through today are marked **Missed — choose on Resume**, not upcoming. The schedule limit includes both skipped and generated dates. The bills are normal bills: review, edit a draft, pay, void and report them through the existing workflows.

## Limits

Two fixed limits apply to every caller, in the core service rather than only on the screens:

- **First bill date: at most 24 months before today.** A new template, or an edit that changes the first bill date, is refused with a message when the date is older. Enter older bills as ordinary bills. A stored date that ages past 24 months stays valid.
- **At most 36 dates per decision or check.** One confirmation, one resume choice and one automatic check each create or skip at most 36 dates for a template. 36 is three years of monthly bills.

A template that has more than 36 dates overdue creates **nothing** automatically; a message names it and asks for Pause, then Resume. This covers books left closed for years and a computer clock set far in the future. Resume then makes the user skip only the excess: with N missed dates, the oldest N − 36 (at most 36 per decision) can only be skipped, and the template stays paused; once 36 remain, Resume offers skip or create as usual. So 40 missed dates mean skipping 4 and choosing for 36; 100 mean skipping 36, then 28, then choosing. Resume counts up to 72 missed dates and says "more than 72" beyond that. An active template in that state is marked on the list and detail pages and cannot be edited until it is paused.

**Make recurring** suggests a first bill date that follows the source bill by whole months and is never older than the 24-month limit.

The limits make large backlogs impossible rather than fast, so the feature carries no bulk-generation optimizations.

## Book format

Migration `0204_recurring_bills` is the single appended migration at ordinal 91 (format 90 → 91). It creates template and occurrence tables, immutable occurrence guards (including skips without bills), retained source-date boundaries, company checks, indexed audit-evidence lookups and open-time verification, and extends shared value/JSON guards. Every template change, cursor advance and book opening validates the template's complete history. It changes no historical migration, existing guard or existing bill row. The exact installed format 90 migration history is pinned in `crates/fb-db/tests/recurring_bills_base_migrations.json`.
