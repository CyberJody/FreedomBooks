-- Job tickets and billable time entries. Not accounting postings: no journal, document identity or seal.
-- Only the draft invoice made from a ticket's entries (through the existing invoice service) affects the books.
-- Four tables, all additive:
--   jobs_ticket     numbered work for one customer (JT-000001), OPEN -> COMPLETED / INVOICED, or CLOSED unbilled.
--   jobs_timeentry  date, customer, service item, exact hours (ten-thousandths), rate (cents), billable flag,
--                   notes, who did the work, optional ticket. amount = half-up(hours * rate / 10000), the invoice
--                   line rule for quantity x rate.
--   jobs_billing    append-only history: which invoice line billed which entry, with the customer, hours, rate and
--                   time value (half-up hours x rate) at billing, and the cents of the line allocated to the entry
--                   (`invoiced`: a line grouping several entries allocates its own rounded amount by cumulative
--                   half-up rounding, so the allocations add up to the line exactly).
--   jobs_billingstep append-only: where a billed entry's line went when a draft invoice was edited (its new position),
--                   or that the line was removed (position NULL), which releases the entry.
-- A billing is live while it was not released and its invoice's current revision is not VOID, so voiding or
-- cancelling the invoice, or removing the line from the draft, releases the time; history is never rewritten.
-- While a billing is live, the invoice keeps its customer and the line keeps its service, quantity, rate and amount:
-- the invoice seal refuses any other revision, so what the time says is billed is what the invoice charges.
-- Guards: company/customer/item/worker scope, entries only join OPEN tickets of their customer, billed entries are
-- frozen, an entry has at most one live billing, billing snapshots equal the entry and its invoice line, a service
-- with recorded time stays a service, and history is permanent.
-- Value guards for every TEXT column, the fb_value_invalid arms, and the open-time evidence view follow.
CREATE TABLE jobs_ticket (
 id TEXT PRIMARY KEY, company_id TEXT NOT NULL REFERENCES ledger_company(id) DEFERRABLE INITIALLY DEFERRED,
 ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 1 AND 999999999999),
 number TEXT NOT NULL CHECK(number='JT-'||printf('%06d',ordinal)),
 request_key TEXT NOT NULL,
 customer_id TEXT NOT NULL REFERENCES directory_customer(id) DEFERRABLE INITIALLY DEFERRED,
 title TEXT NOT NULL CHECK(length(trim(title)) BETWEEN 1 AND 200 AND length(title)<=200),
 description TEXT NOT NULL CHECK(length(description)<=4000),
 status TEXT NOT NULL CHECK(status IN('OPEN','COMPLETED','INVOICED','CLOSED')),
 opened_date TEXT NOT NULL CHECK(date(opened_date) IS opened_date),
 due_date TEXT CHECK(due_date IS NULL OR (date(due_date) IS due_date AND due_date>=opened_date)),
 finished_date TEXT CHECK(finished_date IS NULL OR (date(finished_date) IS finished_date AND finished_date>=opened_date)),
 revision INTEGER NOT NULL CHECK(revision BETWEEN 1 AND 999999999999),
 created_by_id TEXT NOT NULL REFERENCES fb_user(id) DEFERRABLE INITIALLY DEFERRED,
 created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
 CHECK((status='OPEN')=(finished_date IS NULL)),
 CONSTRAINT jobs_ticket_ordinal UNIQUE(company_id,ordinal), CONSTRAINT jobs_ticket_number UNIQUE(company_id,number),
 CONSTRAINT jobs_ticket_request UNIQUE(company_id,request_key)
) STRICT, WITHOUT ROWID;
CREATE INDEX jobs_ticket_customer ON jobs_ticket(company_id,customer_id,status);
CREATE TABLE jobs_timeentry (
 id TEXT PRIMARY KEY, company_id TEXT NOT NULL REFERENCES ledger_company(id) DEFERRABLE INITIALLY DEFERRED,
 request_key TEXT NOT NULL,
 customer_id TEXT NOT NULL REFERENCES directory_customer(id) DEFERRABLE INITIALLY DEFERRED,
 ticket_id TEXT REFERENCES jobs_ticket(id) DEFERRABLE INITIALLY DEFERRED,
 item_id TEXT NOT NULL REFERENCES directory_item(id) DEFERRABLE INITIALLY DEFERRED,
 date TEXT NOT NULL CHECK(date(date) IS date),
 hours INTEGER NOT NULL CHECK(hours BETWEEN 1 AND 10000000),
 rate INTEGER NOT NULL CHECK(rate BETWEEN 0 AND 9999999999),
 amount INTEGER NOT NULL CHECK(amount=(hours*rate+5000)/10000),
 rate_override INTEGER NOT NULL CHECK(rate_override IN(0,1)),
 billable INTEGER NOT NULL CHECK(billable IN(0,1)),
 notes TEXT NOT NULL CHECK(length(notes)<=2000),
 worker_id TEXT NOT NULL REFERENCES fb_user(id) DEFERRABLE INITIALLY DEFERRED,
 revision INTEGER NOT NULL CHECK(revision BETWEEN 1 AND 999999999999),
 created_by_id TEXT NOT NULL REFERENCES fb_user(id) DEFERRABLE INITIALLY DEFERRED,
 created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
 CONSTRAINT jobs_timeentry_request UNIQUE(company_id,request_key)
) STRICT, WITHOUT ROWID;
CREATE INDEX jobs_timeentry_ticket ON jobs_timeentry(ticket_id);
CREATE INDEX jobs_timeentry_customer ON jobs_timeentry(company_id,customer_id,date);
CREATE TABLE jobs_billing (
 id TEXT PRIMARY KEY, company_id TEXT NOT NULL REFERENCES ledger_company(id) DEFERRABLE INITIALLY DEFERRED,
 entry_id TEXT NOT NULL REFERENCES jobs_timeentry(id) DEFERRABLE INITIALLY DEFERRED,
 invoice_id TEXT NOT NULL REFERENCES sales_invoice(id) DEFERRABLE INITIALLY DEFERRED,
 ticket_id TEXT REFERENCES jobs_ticket(id) DEFERRABLE INITIALLY DEFERRED,
 customer_id TEXT NOT NULL REFERENCES directory_customer(id) DEFERRABLE INITIALLY DEFERRED,
 line_position INTEGER NOT NULL CHECK(line_position BETWEEN 1 AND 50),
 hours INTEGER NOT NULL CHECK(hours BETWEEN 1 AND 10000000),
 rate INTEGER NOT NULL CHECK(rate BETWEEN 0 AND 9999999999),
 amount INTEGER NOT NULL CHECK(amount=(hours*rate+5000)/10000),
 invoiced INTEGER NOT NULL CHECK(invoiced>=0 AND invoiced BETWEEN amount-1 AND amount+1),
 created_by_id TEXT NOT NULL REFERENCES fb_user(id) DEFERRABLE INITIALLY DEFERRED,
 created_at TEXT NOT NULL,
 CONSTRAINT jobs_billing_once UNIQUE(entry_id,invoice_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX jobs_billing_entry ON jobs_billing(entry_id);
CREATE INDEX jobs_billing_invoice ON jobs_billing(invoice_id);
CREATE TABLE jobs_billingstep (
 id TEXT PRIMARY KEY, company_id TEXT NOT NULL REFERENCES ledger_company(id) DEFERRABLE INITIALLY DEFERRED,
 billing_id TEXT NOT NULL REFERENCES jobs_billing(id) DEFERRABLE INITIALLY DEFERRED,
 invoice_id TEXT NOT NULL REFERENCES sales_invoice(id) DEFERRABLE INITIALLY DEFERRED,
 revision INTEGER NOT NULL CHECK(revision BETWEEN 2 AND 999999999999),
 line_position INTEGER CHECK(line_position IS NULL OR line_position BETWEEN 1 AND 50),
 created_by_id TEXT NOT NULL REFERENCES fb_user(id) DEFERRABLE INITIALLY DEFERRED,
 created_at TEXT NOT NULL,
 CONSTRAINT jobs_billingstep_once UNIQUE(billing_id,revision)
) STRICT, WITHOUT ROWID;
CREATE INDEX jobs_billingstep_invoice ON jobs_billingstep(invoice_id,revision);
-- Where each billing's line is now: its latest step, else the line it was made on; NULL once released.
CREATE VIEW jobs_billingline AS
 SELECT b.id, b.invoice_id, CASE WHEN EXISTS(SELECT 1 FROM jobs_billingstep s WHERE s.billing_id=b.id)
  THEN (SELECT s.line_position FROM jobs_billingstep s WHERE s.billing_id=b.id ORDER BY s.revision DESC LIMIT 1)
  ELSE b.line_position END AS line_position
 FROM jobs_billing b;
-- A billing is live while its line is still on the invoice and the invoice's current revision is not VOID.
CREATE VIEW jobs_livebilling AS
 SELECT b.id, b.company_id, b.entry_id, b.invoice_id, b.ticket_id, b.customer_id, p.line_position, b.hours, b.rate, b.amount,
  b.invoiced, r.state AS invoice_state, r.id AS revision_id
 FROM jobs_billing b JOIN jobs_billingline p ON p.id=b.id JOIN sales_invoice i ON i.id=b.invoice_id
  JOIN sales_invoicerevision r ON r.invoice_id=i.id AND r.number=i.revision
 WHERE r.state<>'VOID' AND p.line_position IS NOT NULL;
-- Live billings whose invoice no longer charges exactly that time: another customer, or the line at its position is
-- not a charge for the entry's service at the billed rate, for the billed hours together, at the allocated cents.
CREATE VIEW jobs_invoice_mismatch AS
 SELECT l.invoice_id, l.id FROM jobs_livebilling l JOIN sales_invoicerevision r ON r.id=l.revision_id JOIN jobs_timeentry e ON e.id=l.entry_id
 WHERE r.customer_id IS NOT l.customer_id OR NOT EXISTS(SELECT 1 FROM sales_invoiceline n WHERE n.revision_id=r.id AND n.position=l.line_position
  AND n.item_id IS e.item_id AND coalesce(json_extract(n.data,'$.line_kind'),'CHARGE')='CHARGE'
  AND json_extract(n.data,'$.rate') IS printf('%d.%02d',l.rate/100,l.rate%100)
  AND json_extract(n.data,'$.quantity') IS (SELECT printf('%d.%04d',sum(o.hours)/10000,sum(o.hours)%10000) FROM jobs_livebilling o WHERE o.invoice_id=l.invoice_id AND o.line_position=l.line_position)
  AND n.amount IS (SELECT sum(o.invoiced) FROM jobs_livebilling o WHERE o.invoice_id=l.invoice_id AND o.line_position=l.line_position));
CREATE TRIGGER jobs_ticket_insert BEFORE INSERT ON jobs_ticket BEGIN
 SELECT RAISE(ABORT,'jobs_key_collision: Job ticket keys are permanent and cannot be replaced') WHERE EXISTS(SELECT 1 FROM jobs_ticket WHERE id=NEW.id OR (company_id=NEW.company_id AND (ordinal=NEW.ordinal OR number=NEW.number OR request_key=NEW.request_key)));
 SELECT RAISE(ABORT,'jobs_ticket_scope: A job ticket belongs to one customer of its company') WHERE
  NOT EXISTS(SELECT 1 FROM directory_customer WHERE id=NEW.customer_id AND company_id=NEW.company_id)
  OR NOT EXISTS(SELECT 1 FROM ledger_company_members WHERE company_id=NEW.company_id AND user_id=NEW.created_by_id);
 SELECT RAISE(ABORT,'jobs_ticket_number: Job ticket numbers are allocated in order') WHERE
  NEW.ordinal<>1+coalesce((SELECT max(ordinal) FROM jobs_ticket WHERE company_id=NEW.company_id),0);
 SELECT RAISE(ABORT,'jobs_ticket_revision: A new job ticket starts at revision 1') WHERE NEW.revision<>1;
END;
CREATE TRIGGER jobs_ticket_update BEFORE UPDATE ON jobs_ticket BEGIN
 SELECT RAISE(ABORT,'jobs_ticket_identity: A job ticket keeps its company, number, customer and creation') WHERE
  NEW.id IS NOT OLD.id OR NEW.company_id IS NOT OLD.company_id OR NEW.ordinal IS NOT OLD.ordinal OR NEW.number IS NOT OLD.number
  OR NEW.request_key IS NOT OLD.request_key OR NEW.customer_id IS NOT OLD.customer_id
  OR NEW.created_by_id IS NOT OLD.created_by_id OR NEW.created_at IS NOT OLD.created_at;
 SELECT RAISE(ABORT,'jobs_ticket_revision: Each job ticket change takes the next revision') WHERE NEW.revision<>OLD.revision+1;
END;
CREATE TRIGGER jobs_ticket_delete BEFORE DELETE ON jobs_ticket BEGIN
 SELECT RAISE(ABORT,'jobs_ticket_kept: Job tickets are kept. Close the ticket instead.');
END;
CREATE TRIGGER jobs_timeentry_insert BEFORE INSERT ON jobs_timeentry BEGIN
 SELECT RAISE(ABORT,'jobs_key_collision: Time entry keys are permanent and cannot be replaced') WHERE EXISTS(SELECT 1 FROM jobs_timeentry WHERE id=NEW.id OR (company_id=NEW.company_id AND request_key=NEW.request_key));
 SELECT RAISE(ABORT,'jobs_entry_scope: A time entry needs a customer, service item and worker of its company') WHERE
  NOT EXISTS(SELECT 1 FROM directory_customer WHERE id=NEW.customer_id AND company_id=NEW.company_id)
  OR NOT EXISTS(SELECT 1 FROM directory_item WHERE id=NEW.item_id AND company_id=NEW.company_id AND kind='SERVICE')
  OR NOT EXISTS(SELECT 1 FROM ledger_company_members WHERE company_id=NEW.company_id AND user_id=NEW.worker_id)
  OR NOT EXISTS(SELECT 1 FROM ledger_company_members WHERE company_id=NEW.company_id AND user_id=NEW.created_by_id);
 SELECT RAISE(ABORT,'jobs_entry_ticket: Time joins an open job ticket of the same customer') WHERE NEW.ticket_id IS NOT NULL
  AND NOT EXISTS(SELECT 1 FROM jobs_ticket WHERE id=NEW.ticket_id AND company_id=NEW.company_id AND customer_id=NEW.customer_id AND status='OPEN');
 SELECT RAISE(ABORT,'jobs_entry_revision: A new time entry starts at revision 1') WHERE NEW.revision<>1;
END;
CREATE TRIGGER jobs_timeentry_update BEFORE UPDATE ON jobs_timeentry BEGIN
 SELECT RAISE(ABORT,'jobs_entry_billed: This time is on an invoice. Void that invoice to change the entry.') WHERE
  EXISTS(SELECT 1 FROM jobs_livebilling WHERE entry_id=OLD.id);
 SELECT RAISE(ABORT,'jobs_entry_identity: A time entry keeps its company and creation') WHERE
  NEW.id IS NOT OLD.id OR NEW.company_id IS NOT OLD.company_id OR NEW.request_key IS NOT OLD.request_key
  OR NEW.created_by_id IS NOT OLD.created_by_id OR NEW.created_at IS NOT OLD.created_at;
 SELECT RAISE(ABORT,'jobs_entry_scope: A time entry needs a customer, service item and worker of its company') WHERE
  NOT EXISTS(SELECT 1 FROM directory_customer WHERE id=NEW.customer_id AND company_id=NEW.company_id)
  OR NOT EXISTS(SELECT 1 FROM directory_item WHERE id=NEW.item_id AND company_id=NEW.company_id AND kind='SERVICE')
  OR (NEW.worker_id IS NOT OLD.worker_id AND NOT EXISTS(SELECT 1 FROM ledger_company_members WHERE company_id=NEW.company_id AND user_id=NEW.worker_id));
 SELECT RAISE(ABORT,'jobs_entry_ticket: Time joins an open job ticket of the same customer') WHERE NEW.ticket_id IS NOT NULL
  AND (NEW.ticket_id IS NOT OLD.ticket_id OR NEW.customer_id IS NOT OLD.customer_id)
  AND NOT EXISTS(SELECT 1 FROM jobs_ticket WHERE id=NEW.ticket_id AND company_id=NEW.company_id AND customer_id=NEW.customer_id AND status='OPEN');
 SELECT RAISE(ABORT,'jobs_entry_revision: Each time entry change takes the next revision') WHERE NEW.revision<>OLD.revision+1;
END;
CREATE TRIGGER jobs_timeentry_delete BEFORE DELETE ON jobs_timeentry BEGIN
 SELECT RAISE(ABORT,'jobs_entry_history: Time with invoice history is kept. Mark it not billable instead.') WHERE
  EXISTS(SELECT 1 FROM jobs_billing WHERE entry_id=OLD.id);
END;
CREATE TRIGGER jobs_billing_insert BEFORE INSERT ON jobs_billing BEGIN
 SELECT RAISE(ABORT,'jobs_key_collision: Billing history is permanent and cannot be replaced') WHERE EXISTS(SELECT 1 FROM jobs_billing WHERE id=NEW.id OR (entry_id=NEW.entry_id AND invoice_id=NEW.invoice_id));
 SELECT RAISE(ABORT,'jobs_billing_twice: This time is already on an invoice that is not void') WHERE
  EXISTS(SELECT 1 FROM jobs_livebilling WHERE entry_id=NEW.entry_id);
 SELECT RAISE(ABORT,'jobs_billing_entry: Only billable time of the invoice customer, at its recorded hours and rate, is billed') WHERE
  NOT EXISTS(SELECT 1 FROM jobs_timeentry e JOIN sales_invoice i ON i.id=NEW.invoice_id AND i.company_id=e.company_id
   JOIN sales_invoicerevision r ON r.invoice_id=i.id AND r.number=i.revision
   WHERE e.id=NEW.entry_id AND e.company_id=NEW.company_id AND e.billable=1 AND r.state='DRAFT' AND r.number=1
    AND r.customer_id=e.customer_id AND NEW.customer_id=e.customer_id
    AND e.hours=NEW.hours AND e.rate=NEW.rate AND e.amount=NEW.amount AND e.ticket_id IS NEW.ticket_id)
  OR NOT EXISTS(SELECT 1 FROM ledger_company_members WHERE company_id=NEW.company_id AND user_id=NEW.created_by_id);
 SELECT RAISE(ABORT,'jobs_billing_line: Billed time goes on a charge line for its service, rate and hours') WHERE
  NOT EXISTS(SELECT 1 FROM jobs_timeentry e JOIN sales_invoice i ON i.id=NEW.invoice_id JOIN sales_invoicerevision r ON r.invoice_id=i.id AND r.number=i.revision
   JOIN sales_invoiceline n ON n.revision_id=r.id AND n.position=NEW.line_position
   WHERE e.id=NEW.entry_id AND n.item_id IS e.item_id AND coalesce(json_extract(n.data,'$.line_kind'),'CHARGE')='CHARGE'
    AND json_extract(n.data,'$.rate') IS printf('%d.%02d',NEW.rate/100,NEW.rate%100)
    AND NEW.hours+coalesce((SELECT sum(o.hours) FROM jobs_billing o WHERE o.invoice_id=NEW.invoice_id AND o.line_position=NEW.line_position),0)
     <=CAST(replace(json_extract(n.data,'$.quantity'),'.','') AS INTEGER)
    AND NEW.invoiced+coalesce((SELECT sum(o.invoiced) FROM jobs_billing o WHERE o.invoice_id=NEW.invoice_id AND o.line_position=NEW.line_position),0)<=n.amount);
END;
CREATE TRIGGER jobs_billing_update BEFORE UPDATE ON jobs_billing BEGIN
 SELECT RAISE(ABORT,'jobs_billing_immutable: Billing history is permanent');
END;
CREATE TRIGGER jobs_billing_delete BEFORE DELETE ON jobs_billing BEGIN
 SELECT RAISE(ABORT,'jobs_billing_immutable: Billing history is permanent');
END;
-- A step is written while a draft invoice is edited, for the revision being written (before its row exists), and
-- only for time still billed on that draft. The seal of that revision then checks the result.
CREATE TRIGGER jobs_billingstep_insert BEFORE INSERT ON jobs_billingstep BEGIN
 SELECT RAISE(ABORT,'jobs_key_collision: Billing history is permanent and cannot be replaced') WHERE EXISTS(SELECT 1 FROM jobs_billingstep WHERE id=NEW.id OR (billing_id=NEW.billing_id AND revision=NEW.revision));
 SELECT RAISE(ABORT,'jobs_billing_step: Only time still billed on a draft invoice follows that invoice''s next revision') WHERE
  NOT EXISTS(SELECT 1 FROM jobs_livebilling l JOIN sales_invoice i ON i.id=l.invoice_id
   WHERE l.id=NEW.billing_id AND l.company_id=NEW.company_id AND l.invoice_id=NEW.invoice_id AND l.invoice_state='DRAFT' AND NEW.revision=i.revision+1)
  OR EXISTS(SELECT 1 FROM sales_invoicerevision WHERE invoice_id=NEW.invoice_id AND number=NEW.revision)
  OR NOT EXISTS(SELECT 1 FROM ledger_company_members WHERE company_id=NEW.company_id AND user_id=NEW.created_by_id);
END;
CREATE TRIGGER jobs_billingstep_update BEFORE UPDATE ON jobs_billingstep BEGIN
 SELECT RAISE(ABORT,'jobs_billing_immutable: Billing history is permanent');
END;
CREATE TRIGGER jobs_billingstep_delete BEFORE DELETE ON jobs_billingstep BEGIN
 SELECT RAISE(ABORT,'jobs_billing_immutable: Billing history is permanent');
END;
-- Every invoice revision except a void keeps charging the time still billed on it.
CREATE TRIGGER jobs_invoice_seal BEFORE INSERT ON sales_seal WHEN NEW.kind='sales_invoice_revision' BEGIN
 SELECT RAISE(ABORT,'jobs_invoice_time: This invoice bills recorded time. Keep the customer and each time line''s service, quantity and rate, or remove the time lines to release that time.')
  WHERE EXISTS(SELECT 1 FROM sales_invoicerevision r JOIN jobs_invoice_mismatch m ON m.invoice_id=r.invoice_id WHERE r.id=NEW.doc_id AND r.state<>'VOID');
END;
-- A service with recorded time stays a service: time entries and their open-time check require it.
CREATE TRIGGER jobs_item_kind BEFORE UPDATE OF kind ON directory_item WHEN OLD.kind='SERVICE' AND NEW.kind IS NOT 'SERVICE' BEGIN
 SELECT RAISE(ABORT,'jobs_item_kind: Time is recorded against this service, so it stays a service. Create a separate product instead.')
  WHERE EXISTS(SELECT 1 FROM jobs_timeentry WHERE item_id=OLD.id);
END;
-- Open-time evidence: the same rules, for books that a connection without these triggers could have changed.
CREATE VIEW jobs_invalid AS
 SELECT t.id FROM jobs_ticket t WHERE t.number<>'JT-'||printf('%06d',t.ordinal)
  OR NOT EXISTS(SELECT 1 FROM directory_customer c WHERE c.id=t.customer_id AND c.company_id=t.company_id)
 UNION ALL SELECT e.id FROM jobs_timeentry e WHERE e.amount<>(e.hours*e.rate+5000)/10000
  OR NOT EXISTS(SELECT 1 FROM directory_customer c WHERE c.id=e.customer_id AND c.company_id=e.company_id)
  OR NOT EXISTS(SELECT 1 FROM directory_item i WHERE i.id=e.item_id AND i.company_id=e.company_id AND i.kind='SERVICE')
  OR (e.ticket_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM jobs_ticket t WHERE t.id=e.ticket_id AND t.company_id=e.company_id AND t.customer_id=e.customer_id))
 UNION ALL SELECT b.id FROM jobs_billing b LEFT JOIN jobs_timeentry e ON e.id=b.entry_id LEFT JOIN sales_invoice i ON i.id=b.invoice_id
  WHERE e.id IS NULL OR i.id IS NULL OR e.company_id<>b.company_id OR i.company_id<>b.company_id
  OR NOT EXISTS(SELECT 1 FROM directory_customer c WHERE c.id=b.customer_id AND c.company_id=b.company_id)
  OR NOT EXISTS(SELECT 1 FROM sales_invoicerevision r WHERE r.invoice_id=i.id AND r.number=1 AND r.customer_id=b.customer_id)
 UNION ALL SELECT s.id FROM jobs_billingstep s LEFT JOIN jobs_billing b ON b.id=s.billing_id
  WHERE b.id IS NULL OR b.invoice_id<>s.invoice_id OR b.company_id<>s.company_id
  OR NOT EXISTS(SELECT 1 FROM sales_invoicerevision r WHERE r.invoice_id=s.invoice_id AND r.number=s.revision AND json_extract(r.data,'$.action')='revised')
  OR EXISTS(SELECT 1 FROM jobs_billingstep x WHERE x.billing_id=s.billing_id AND x.line_position IS NULL AND x.revision<s.revision)
 UNION ALL SELECT l.id FROM jobs_livebilling l JOIN jobs_timeentry e ON e.id=l.entry_id
  WHERE e.billable<>1 OR e.hours<>l.hours OR e.rate<>l.rate OR e.amount<>l.amount OR e.ticket_id IS NOT l.ticket_id OR e.customer_id<>l.customer_id
 UNION ALL SELECT id FROM jobs_invoice_mismatch
 UNION ALL SELECT entry_id FROM jobs_livebilling GROUP BY entry_id HAVING count(*)>1;
-- GENERATED BELOW by crates/fb-db/tests/generate_job_ticket_guards.py: value guards and fb_value_invalid.
CREATE TRIGGER fb_valid_jobs_billing_insert BEFORE INSERT ON jobs_billing WHEN NOT fb_valid_uuid(NEW.id) OR NOT fb_valid_uuid(NEW.company_id) OR NOT fb_valid_uuid(NEW.entry_id) OR NOT fb_valid_uuid(NEW.invoice_id) OR NOT fb_valid_uuid(NEW.ticket_id) OR NOT fb_valid_uuid(NEW.customer_id) OR NOT fb_valid_uuid(NEW.created_by_id) OR NOT fb_valid_timestamp(NEW.created_at) BEGIN SELECT RAISE(ABORT,'fb_valid: Invalid stored value'); END;
CREATE TRIGGER fb_valid_jobs_billing_update BEFORE UPDATE ON jobs_billing WHEN NOT fb_valid_uuid(NEW.id) OR NOT fb_valid_uuid(NEW.company_id) OR NOT fb_valid_uuid(NEW.entry_id) OR NOT fb_valid_uuid(NEW.invoice_id) OR NOT fb_valid_uuid(NEW.ticket_id) OR NOT fb_valid_uuid(NEW.customer_id) OR NOT fb_valid_uuid(NEW.created_by_id) OR NOT fb_valid_timestamp(NEW.created_at) BEGIN SELECT RAISE(ABORT,'fb_valid: Invalid stored value'); END;
CREATE TRIGGER fb_valid_jobs_billingstep_insert BEFORE INSERT ON jobs_billingstep WHEN NOT fb_valid_uuid(NEW.id) OR NOT fb_valid_uuid(NEW.company_id) OR NOT fb_valid_uuid(NEW.billing_id) OR NOT fb_valid_uuid(NEW.invoice_id) OR NOT fb_valid_uuid(NEW.created_by_id) OR NOT fb_valid_timestamp(NEW.created_at) BEGIN SELECT RAISE(ABORT,'fb_valid: Invalid stored value'); END;
CREATE TRIGGER fb_valid_jobs_billingstep_update BEFORE UPDATE ON jobs_billingstep WHEN NOT fb_valid_uuid(NEW.id) OR NOT fb_valid_uuid(NEW.company_id) OR NOT fb_valid_uuid(NEW.billing_id) OR NOT fb_valid_uuid(NEW.invoice_id) OR NOT fb_valid_uuid(NEW.created_by_id) OR NOT fb_valid_timestamp(NEW.created_at) BEGIN SELECT RAISE(ABORT,'fb_valid: Invalid stored value'); END;
CREATE TRIGGER fb_valid_jobs_ticket_insert BEFORE INSERT ON jobs_ticket WHEN NOT fb_valid_uuid(NEW.id) OR NOT fb_valid_uuid(NEW.company_id) OR NOT fb_valid_text(NEW.number) OR NOT fb_valid_uuid(NEW.request_key) OR NOT fb_valid_uuid(NEW.customer_id) OR NOT fb_valid_text(NEW.title) OR NOT fb_valid_text(NEW.description) OR NOT fb_valid_text(NEW.status) OR NOT fb_valid_date(NEW.opened_date) OR NOT fb_valid_date(NEW.due_date) OR NOT fb_valid_date(NEW.finished_date) OR NOT fb_valid_uuid(NEW.created_by_id) OR NOT fb_valid_timestamp(NEW.created_at) OR NOT fb_valid_timestamp(NEW.updated_at) BEGIN SELECT RAISE(ABORT,'fb_valid: Invalid stored value'); END;
CREATE TRIGGER fb_valid_jobs_ticket_update BEFORE UPDATE ON jobs_ticket WHEN NOT fb_valid_uuid(NEW.id) OR NOT fb_valid_uuid(NEW.company_id) OR NOT fb_valid_text(NEW.number) OR NOT fb_valid_uuid(NEW.request_key) OR NOT fb_valid_uuid(NEW.customer_id) OR NOT fb_valid_text(NEW.title) OR NOT fb_valid_text(NEW.description) OR NOT fb_valid_text(NEW.status) OR NOT fb_valid_date(NEW.opened_date) OR NOT fb_valid_date(NEW.due_date) OR NOT fb_valid_date(NEW.finished_date) OR NOT fb_valid_uuid(NEW.created_by_id) OR NOT fb_valid_timestamp(NEW.created_at) OR NOT fb_valid_timestamp(NEW.updated_at) BEGIN SELECT RAISE(ABORT,'fb_valid: Invalid stored value'); END;
CREATE TRIGGER fb_valid_jobs_timeentry_insert BEFORE INSERT ON jobs_timeentry WHEN NOT fb_valid_uuid(NEW.id) OR NOT fb_valid_uuid(NEW.company_id) OR NOT fb_valid_uuid(NEW.request_key) OR NOT fb_valid_uuid(NEW.customer_id) OR NOT fb_valid_uuid(NEW.ticket_id) OR NOT fb_valid_uuid(NEW.item_id) OR NOT fb_valid_date(NEW.date) OR NOT fb_valid_text(NEW.notes) OR NOT fb_valid_uuid(NEW.worker_id) OR NOT fb_valid_uuid(NEW.created_by_id) OR NOT fb_valid_timestamp(NEW.created_at) OR NOT fb_valid_timestamp(NEW.updated_at) BEGIN SELECT RAISE(ABORT,'fb_valid: Invalid stored value'); END;
CREATE TRIGGER fb_valid_jobs_timeentry_update BEFORE UPDATE ON jobs_timeentry WHEN NOT fb_valid_uuid(NEW.id) OR NOT fb_valid_uuid(NEW.company_id) OR NOT fb_valid_uuid(NEW.request_key) OR NOT fb_valid_uuid(NEW.customer_id) OR NOT fb_valid_uuid(NEW.ticket_id) OR NOT fb_valid_uuid(NEW.item_id) OR NOT fb_valid_date(NEW.date) OR NOT fb_valid_text(NEW.notes) OR NOT fb_valid_uuid(NEW.worker_id) OR NOT fb_valid_uuid(NEW.created_by_id) OR NOT fb_valid_timestamp(NEW.created_at) OR NOT fb_valid_timestamp(NEW.updated_at) BEGIN SELECT RAISE(ABORT,'fb_valid: Invalid stored value'); END;
DROP VIEW fb_value_invalid;
CREATE VIEW fb_value_invalid AS
SELECT 'invalid stored value in accounts_owner' AS problem FROM accounts_owner WHERE
 NOT fb_valid_uuid(user_id)
UNION ALL
SELECT 'invalid stored value in assistant_connection' AS problem FROM assistant_connection WHERE
 NOT fb_valid_text(account_email) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(created_by_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(key_last4) OR
 NOT fb_valid_text(last_error) OR
 NOT fb_valid_text(mode) OR
 NOT fb_valid_text(model) OR
 NOT fb_valid_text(model_choices) OR
 NOT fb_valid_text(status)
UNION ALL
SELECT 'invalid stored value in assistant_reading' AS problem FROM assistant_reading WHERE
 NOT fb_valid_uuid(bill_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(content_type) OR
 NOT fb_valid_uuid(created_by_id) OR
 NOT fb_valid_text(error) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(mode) OR
 NOT fb_valid_text(model) OR
 NOT fb_valid_uuid(origin_bill_id) OR
 NOT fb_valid_text(original_name) OR
 NOT fb_valid_uuid(request_key) OR
 NOT fb_valid_text(result) OR
 NOT fb_valid_text(sha256) OR
 NOT fb_valid_uuid(source_file_id) OR
 NOT fb_valid_text(status)
UNION ALL
SELECT 'invalid stored value in attachments_attachment' AS problem FROM attachments_attachment WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(content_type) OR
 NOT fb_valid_text(delete_reason) OR
 NOT fb_valid_timestamp(deleted_at) OR
 NOT fb_valid_uuid(deleted_by_id) OR
 NOT fb_valid_text(deleted_by_name) OR
 NOT fb_valid_text(deleted_number) OR
 NOT fb_valid_uuid(document_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_text(nonce) OR
 NOT fb_valid_text(original_name) OR
 NOT fb_valid_text(original_sha256) OR
 NOT fb_valid_uuid(request_key) OR
 NOT fb_valid_text(sha256) OR
 NOT fb_valid_text(stored_name) OR
 NOT fb_valid_timestamp(uploaded_at) OR
 NOT fb_valid_uuid(uploaded_by_id) OR
 NOT fb_valid_text(uploaded_by_name) OR
 NOT fb_valid_text(uploaded_number) OR
 NOT fb_valid_text(wrap_nonce) OR
 NOT fb_valid_text(wrapped_key)
UNION ALL
SELECT 'invalid stored value in bankfeeds_account' AS problem FROM bankfeeds_account WHERE
 NOT fb_valid_timestamp(balance_at) OR
 NOT fb_valid_text(conn_key) OR
 NOT fb_valid_text(conn_key_identity) OR
 NOT fb_valid_uuid(connection_id) OR
 NOT fb_valid_text(currency) OR
 NOT fb_valid_timestamp(first_seen_at) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_timestamp(last_seen_at) OR
 NOT fb_valid_uuid(ledger_account_id) OR
 NOT fb_valid_timestamp(mapped_at) OR
 NOT fb_valid_text(name) OR
 NOT fb_valid_text(org_name) OR
 NOT fb_valid_text(remote_id) OR
 NOT fb_valid_text(remote_identity)
UNION ALL
SELECT 'invalid stored value in bankfeeds_connection' AS problem FROM bankfeeds_connection WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_uuid(created_by_id) OR
 NOT fb_valid_timestamp(forgotten_at) OR
 NOT fb_valid_text(host) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(label) OR
 NOT fb_valid_timestamp(last_sync_at) OR
 NOT fb_valid_text(last_sync_summary)
UNION ALL
SELECT 'invalid stored value in bankfeeds_counter' AS problem FROM bankfeeds_counter WHERE
 NOT fb_valid_text(name)
UNION ALL
SELECT 'invalid stored value in bankfeeds_identity_secret' AS problem FROM bankfeeds_identity_secret WHERE
 NOT fb_valid_text(name)
UNION ALL
SELECT 'invalid stored value in bankfeeds_retired' AS problem FROM bankfeeds_retired WHERE
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in bankfeeds_rule' AS problem FROM bankfeeds_rule WHERE
 NOT fb_valid_uuid(account_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(match_text) OR
 NOT fb_valid_text(name) OR
 NOT fb_valid_text(payee_name) OR
 NOT fb_valid_text(sign) OR
 NOT fb_valid_timestamp(updated_at)
UNION ALL
SELECT 'invalid stored value in bankfeeds_transaction' AS problem FROM bankfeeds_transaction WHERE
 NOT fb_valid_text(accepted_as) OR
 NOT fb_valid_timestamp(decided_at) OR
 NOT fb_valid_uuid(decided_by_id) OR
 NOT fb_valid_text(description) OR
 NOT fb_valid_uuid(feed_account_id) OR
 NOT fb_valid_timestamp(first_seen_at) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_uuid(journal_line_id) OR
 NOT fb_valid_text(posted_on) OR
 NOT fb_valid_text(reference) OR
 NOT fb_valid_text(reference_url) OR
 NOT fb_valid_text(remote_id) OR
 NOT fb_valid_text(remote_identity) OR
 NOT fb_valid_text(status) OR
 NOT fb_valid_text(transacted_on) OR
 NOT fb_valid_timestamp(updated_at)
UNION ALL
SELECT 'invalid stored value in banking_banklinestatus' AS problem FROM banking_banklinestatus WHERE
 NOT fb_valid_uuid(account_id) OR
 NOT fb_valid_uuid(changed_by_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_line_id) OR
 NOT fb_valid_uuid(reconciliation_id) OR
 NOT fb_valid_text(state)
UNION ALL
SELECT 'invalid stored value in banking_deposit' AS problem FROM banking_deposit WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(bank_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id) OR
 NOT fb_valid_uuid(journal_id)
UNION ALL
SELECT 'invalid stored value in banking_depositline' AS problem FROM banking_depositline WHERE
 NOT fb_valid_uuid(deposit_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(receipt_id) OR
 NOT fb_valid_uuid(undeposited_id)
UNION ALL
SELECT 'invalid stored value in banking_depositreversal' AS problem FROM banking_depositreversal WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(deposit_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id)
UNION ALL
SELECT 'invalid stored value in banking_reconciliation' AS problem FROM banking_reconciliation WHERE
 NOT fb_valid_uuid(account_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(creation_digest) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(request_key) OR
 NOT fb_valid_uuid(started_by_id) OR
 NOT fb_valid_date(statement_date) OR
 NOT fb_valid_text(status)
UNION ALL
SELECT 'invalid stored value in banking_reconciliationactor' AS problem FROM banking_reconciliationactor WHERE
 NOT fb_valid_text(phase) OR
 NOT fb_valid_uuid(reconciliation_id) OR
 NOT fb_valid_uuid(user_id)
UNION ALL
SELECT 'invalid stored value in banking_reconciliationline' AS problem FROM banking_reconciliationline WHERE
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_line_id) OR
 NOT fb_valid_uuid(reconciliation_id)
UNION ALL
SELECT 'invalid stored value in banking_seal' AS problem FROM banking_seal WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in banking_seal_due' AS problem FROM banking_seal_due WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in checks_check' AS problem FROM checks_check WHERE
 NOT fb_valid_uuid(bank_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(creation_digest) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id) OR
 NOT fb_valid_text(number_key) OR
 NOT fb_valid_text(state) OR
 NOT fb_valid_uuid(vendor_id)
UNION ALL
SELECT 'invalid stored value in checks_checkline' AS problem FROM checks_checkline WHERE
 NOT fb_valid_uuid(account_id) OR
 NOT fb_valid_text(account_snapshot) OR
 NOT fb_valid_text(description) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(item_id) OR
 NOT fb_valid_text(item_snapshot) OR
 NOT fb_valid_uuid(revision_id) OR
 NOT fb_valid_text(tab)
UNION ALL
SELECT 'invalid stored value in checks_checkrevision' AS problem FROM checks_checkrevision WHERE
 NOT fb_valid_text(action) OR
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(actor_name) OR
 NOT fb_valid_text(address) OR
 NOT fb_valid_uuid(bank_id) OR
 NOT fb_valid_text(bank_snapshot) OR
 NOT fb_valid_text(check_number) OR
 NOT fb_valid_text(company_snapshot) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(document_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_text(memo) OR
 NOT fb_valid_text(payee_kind) OR
 NOT fb_valid_text(payee_name) OR
 NOT fb_valid_text(payee_snapshot) OR
 NOT fb_valid_text(reason) OR
 NOT fb_valid_uuid(reversal_id) OR
 NOT fb_valid_text(review_digest) OR
 NOT fb_valid_text(state) OR
 NOT fb_valid_uuid(vendor_id)
UNION ALL
SELECT 'invalid stored value in credits_creditapplication' AS problem FROM credits_creditapplication WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(credit_memo_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(request_key)
UNION ALL
SELECT 'invalid stored value in credits_creditapplicationline' AS problem FROM credits_creditapplicationline WHERE
 NOT fb_valid_uuid(application_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(invoice_id) OR
 NOT fb_valid_uuid(invoice_revision_id) OR
 NOT fb_valid_uuid(receivables_id)
UNION ALL
SELECT 'invalid stored value in credits_creditmemo' AS problem FROM credits_creditmemo WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(collected_tax_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(customer_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_uuid(receivables_id) OR
 NOT fb_valid_uuid(tax_item_id)
UNION ALL
SELECT 'invalid stored value in credits_creditmemoline' AS problem FROM credits_creditmemoline WHERE
 NOT fb_valid_uuid(credit_memo_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(income_account_id) OR
 NOT fb_valid_uuid(item_id)
UNION ALL
SELECT 'invalid stored value in credits_creditmemovoid' AS problem FROM credits_creditmemovoid WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(credit_memo_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id)
UNION ALL
SELECT 'invalid stored value in credits_creditrefund' AS problem FROM credits_creditrefund WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(bank_id) OR
 NOT fb_valid_text(check_number) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(credit_memo_id) OR
 NOT fb_valid_uuid(customer_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_text(method) OR
 NOT fb_valid_uuid(receivables_id)
UNION ALL
SELECT 'invalid stored value in credits_creditrefundvoid' AS problem FROM credits_creditrefundvoid WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_uuid(refund_id)
UNION ALL
SELECT 'invalid stored value in credits_creditunapplication' AS problem FROM credits_creditunapplication WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(application_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id)
UNION ALL
SELECT 'invalid stored value in credits_seal' AS problem FROM credits_seal WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in credits_seal_due' AS problem FROM credits_seal_due WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in directory_customer' AS problem FROM directory_customer WHERE
 NOT fb_valid_text(billing_address) OR
 NOT fb_valid_text(business_name) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(contact_name) OR
 NOT fb_valid_text(display_name) OR
 NOT fb_valid_text(email) OR
 NOT fb_valid_text(exemption_number) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(notes) OR
 NOT fb_valid_text(phone) OR
 NOT fb_valid_text(tax_status)
UNION ALL
SELECT 'invalid stored value in directory_item' AS problem FROM directory_item WHERE
 NOT fb_valid_text(code) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(description) OR
 NOT fb_valid_uuid(expense_account_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(income_account_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in directory_vendor' AS problem FROM directory_vendor WHERE
 NOT fb_valid_text(billing_address) OR
 NOT fb_valid_text(business_name) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(contact_name) OR
 NOT fb_valid_uuid(default_expense_account_id) OR
 NOT fb_valid_text(default_terms) OR
 NOT fb_valid_text(display_name) OR
 NOT fb_valid_text(email) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(notes) OR
 NOT fb_valid_text(phone)
UNION ALL
SELECT 'invalid stored value in documents_accountingsetup' AS problem FROM documents_accountingsetup WHERE
 NOT fb_valid_uuid(collected_tax_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(customer_deposits_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(payables_id) OR
 NOT fb_valid_uuid(receivables_id) OR
 NOT fb_valid_uuid(undeposited_id)
UNION ALL
SELECT 'invalid stored value in documents_designatedbank' AS problem FROM documents_designatedbank WHERE
 NOT fb_valid_uuid(account_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(setup_id)
UNION ALL
SELECT 'invalid stored value in documents_documentaccount' AS problem FROM documents_documentaccount WHERE
 NOT fb_valid_text(account_code) OR
 NOT fb_valid_uuid(account_id) OR
 NOT fb_valid_text(account_kind) OR
 NOT fb_valid_text(account_name) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(role) OR
 NOT fb_valid_uuid(source_id)
UNION ALL
SELECT 'invalid stored value in documents_documentidentity' AS problem FROM documents_documentidentity WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_text(currency) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(kind) OR
 NOT fb_valid_text(number) OR
 NOT fb_valid_uuid(request_key)
UNION ALL
SELECT 'invalid stored value in documents_documentsequence' AS problem FROM documents_documentsequence WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in estimate_seal' AS problem FROM estimate_seal WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in estimate_seal_due' AS problem FROM estimate_seal_due WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in fb_seal' AS problem FROM fb_seal WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in fb_seal_due' AS problem FROM fb_seal_due WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in fb_user' AS problem FROM fb_user WHERE
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(username)
UNION ALL
SELECT 'invalid stored value in ledger_account' AS problem FROM ledger_account WHERE
 NOT fb_valid_text(code) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(description) OR
 NOT fb_valid_text(detail_type) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(kind) OR
 NOT fb_valid_text(name)
UNION ALL
SELECT 'invalid stored value in ledger_auditevent' AS problem FROM ledger_auditevent WHERE
 NOT fb_valid_text(action) OR
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(actor_name) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(detail) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_timestamp(occurred_at) OR
 NOT fb_valid_text(target)
UNION ALL
SELECT 'invalid stored value in ledger_company' AS problem FROM ledger_company WHERE
 NOT fb_valid_text(currency) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(name)
UNION ALL
SELECT 'invalid stored value in ledger_company_members' AS problem FROM ledger_company_members WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(user_id)
UNION ALL
SELECT 'invalid stored value in ledger_journal' AS problem FROM ledger_journal WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_uuid(created_by_id) OR
 NOT fb_valid_text(creation_digest) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_text(entry_number) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(memo) OR
 NOT fb_valid_timestamp(posted_at) OR
 NOT fb_valid_uuid(posted_by_id) OR
 NOT fb_valid_uuid(request_key) OR
 NOT fb_valid_uuid(reversal_of_id) OR
 NOT fb_valid_uuid(source_id) OR
 NOT fb_valid_text(status)
UNION ALL
SELECT 'invalid stored value in ledger_journalline' AS problem FROM ledger_journalline WHERE
 NOT fb_valid_uuid(account_id) OR
 NOT fb_valid_text(description) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_text(name)
UNION ALL
SELECT 'invalid stored value in payables_bill' AS problem FROM payables_bill WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(creation_digest) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id) OR
 NOT fb_valid_text(reference_key) OR
 NOT fb_valid_text(state) OR
 NOT fb_valid_uuid(vendor_id)
UNION ALL
SELECT 'invalid stored value in payables_billline' AS problem FROM payables_billline WHERE
 NOT fb_valid_text(account_snapshot) OR
 NOT fb_valid_text(description) OR
 NOT fb_valid_uuid(expense_account_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(income_account_id) OR
 NOT fb_valid_uuid(item_id) OR
 NOT fb_valid_text(item_snapshot) OR
 NOT fb_valid_uuid(revision_id)
UNION ALL
SELECT 'invalid stored value in payables_billpayment' AS problem FROM payables_billpayment WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(actor_name) OR
 NOT fb_valid_uuid(bank_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(company_snapshot) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_text(creation_digest) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_text(memo) OR
 NOT fb_valid_text(method) OR
 NOT fb_valid_text(reference) OR
 NOT fb_valid_text(review_digest) OR
 NOT fb_valid_uuid(vendor_id) OR
 NOT fb_valid_text(vendor_snapshot)
UNION ALL
SELECT 'invalid stored value in payables_billrevision' AS problem FROM payables_billrevision WHERE
 NOT fb_valid_text(action) OR
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(actor_name) OR
 NOT fb_valid_uuid(bill_id) OR
 NOT fb_valid_text(company_snapshot) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_date(due_date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_text(memo) OR
 NOT fb_valid_text(reason) OR
 NOT fb_valid_text(reentry_reason) OR
 NOT fb_valid_text(reference) OR
 NOT fb_valid_uuid(reversal_id) OR
 NOT fb_valid_text(review_digest) OR
 NOT fb_valid_text(state) OR
 NOT fb_valid_uuid(supersedes_id) OR
 NOT fb_valid_uuid(vendor_id) OR
 NOT fb_valid_text(vendor_snapshot)
UNION ALL
SELECT 'invalid stored value in payables_paymentallocation' AS problem FROM payables_paymentallocation WHERE
 NOT fb_valid_uuid(bill_id) OR
 NOT fb_valid_uuid(bill_revision_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(payables_id) OR
 NOT fb_valid_uuid(payment_id)
UNION ALL
SELECT 'invalid stored value in payables_paymentreversal' AS problem FROM payables_paymentreversal WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(actor_name) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_uuid(payment_id) OR
 NOT fb_valid_text(reason)
UNION ALL
SELECT 'invalid stored value in plugin_credential' AS problem FROM plugin_credential WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(plugin_id) OR
 NOT fb_valid_uuid(connection_id)
UNION ALL
SELECT 'invalid stored value in prepayments_customerdeposit' AS problem FROM prepayments_customerdeposit WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(estimate_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(liability_id) OR
 NOT fb_valid_text(order_reference) OR
 NOT fb_valid_uuid(receipt_id)
UNION ALL
SELECT 'invalid stored value in prepayments_depositapplication' AS problem FROM prepayments_depositapplication WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(deposit_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id) OR
 NOT fb_valid_uuid(invoice_id) OR
 NOT fb_valid_uuid(invoice_revision_id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_uuid(liability_id) OR
 NOT fb_valid_uuid(receivables_id)
UNION ALL
SELECT 'invalid stored value in prepayments_depositrefund' AS problem FROM prepayments_depositrefund WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(bank_id) OR
 NOT fb_valid_text(check_number) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(customer_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(deposit_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_uuid(liability_id) OR
 NOT fb_valid_text(method)
UNION ALL
SELECT 'invalid stored value in prepayments_depositrefundvoid' AS problem FROM prepayments_depositrefundvoid WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_uuid(refund_id)
UNION ALL
SELECT 'invalid stored value in prepayments_depositunapplication' AS problem FROM prepayments_depositunapplication WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(application_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id)
UNION ALL
SELECT 'invalid stored value in prepayments_due' AS problem FROM prepayments_due WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in prepayments_seal' AS problem FROM prepayments_seal WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in sales_estimate' AS problem FROM sales_estimate WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(creation_digest) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id)
UNION ALL
SELECT 'invalid stored value in sales_estimateconversion' AS problem FROM sales_estimateconversion WHERE
 NOT fb_valid_text(digest) OR
 NOT fb_valid_uuid(estimate_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(invoice_id) OR
 NOT fb_valid_uuid(revision_id)
UNION ALL
SELECT 'invalid stored value in sales_estimateline' AS problem FROM sales_estimateline WHERE
 NOT fb_valid_text(data) OR
 NOT fb_valid_uuid(expense_account_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(income_account_id) OR
 NOT fb_valid_uuid(item_id) OR
 NOT fb_valid_uuid(revision_id)
UNION ALL
SELECT 'invalid stored value in sales_estimaterevision' AS problem FROM sales_estimaterevision WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(customer_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(estimate_id) OR
 NOT fb_valid_text(expiry) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(state)
UNION ALL
SELECT 'invalid stored value in sales_invoice' AS problem FROM sales_invoice WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(creation_digest) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id)
UNION ALL
SELECT 'invalid stored value in sales_invoiceline' AS problem FROM sales_invoiceline WHERE
 NOT fb_valid_text(data) OR
 NOT fb_valid_uuid(expense_account_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(income_account_id) OR
 NOT fb_valid_uuid(item_id) OR
 NOT fb_valid_uuid(revision_id)
UNION ALL
SELECT 'invalid stored value in sales_invoicerevision' AS problem FROM sales_invoicerevision WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(customer_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_date(due_date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(invoice_id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_uuid(reversal_id) OR
 NOT fb_valid_text(state)
UNION ALL
SELECT 'invalid stored value in sales_receipt' AS problem FROM sales_receipt WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(customer_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_text(destination) OR
 NOT fb_valid_uuid(destination_account_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in sales_receiptallocation' AS problem FROM sales_receiptallocation WHERE
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(invoice_id) OR
 NOT fb_valid_uuid(invoice_revision_id) OR
 NOT fb_valid_uuid(receipt_id) OR
 NOT fb_valid_uuid(receivables_id)
UNION ALL
SELECT 'invalid stored value in sales_receiptreversal' AS problem FROM sales_receiptreversal WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_uuid(receipt_id)
UNION ALL
SELECT 'invalid stored value in sales_salesreceipt' AS problem FROM sales_salesreceipt WHERE
 NOT fb_valid_text(data) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(receipt_id)
UNION ALL
SELECT 'invalid stored value in sales_salesreceiptline' AS problem FROM sales_salesreceiptline WHERE
 NOT fb_valid_text(data) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(income_account_id) OR
 NOT fb_valid_uuid(item_id) OR
 NOT fb_valid_uuid(sale_id)
UNION ALL
SELECT 'invalid stored value in sales_seal' AS problem FROM sales_seal WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in sales_seal_due' AS problem FROM sales_seal_due WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in salestax_salestaxsettings' AS problem FROM salestax_salestaxsettings WHERE
 NOT fb_valid_text(bulk_step) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(default_item_id) OR
 NOT fb_valid_text(filing_frequency) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(owed_basis)
UNION ALL
SELECT 'invalid stored value in salestax_taxgroupmember' AS problem FROM salestax_taxgroupmember WHERE
 NOT fb_valid_uuid(group_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(member_id)
UNION ALL
SELECT 'invalid stored value in salestax_taxitem' AS problem FROM salestax_taxitem WHERE
 NOT fb_valid_uuid(agency_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(description) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(kind) OR
 NOT fb_valid_text(name)
UNION ALL
SELECT 'invalid stored value in transfers_transfer' AS problem FROM transfers_transfer WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(actor_name) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(company_snapshot) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_text(creation_digest) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(from_account_id) OR
 NOT fb_valid_text(from_snapshot) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_text(memo) OR
 NOT fb_valid_text(review_digest) OR
 NOT fb_valid_uuid(to_account_id) OR
 NOT fb_valid_text(to_snapshot)
UNION ALL
SELECT 'invalid stored value in transfers_transfervoid' AS problem FROM transfers_transfervoid WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(actor_name) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_text(reason) OR
 NOT fb_valid_uuid(transfer_id)
UNION ALL SELECT 'invalid stored value in company_document_details' AS problem FROM company_document_details WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_text(logo_mime) OR (fb_json_unique_keys(data) AND NOT fb_company_document_valid(data))
UNION ALL
SELECT 'invalid stored value in fb_numbering' AS problem FROM fb_numbering WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(kind) OR
 NOT fb_valid_text(scope) OR
 NOT fb_valid_text(start) OR
 NOT fb_valid_uuid(audit_id)
UNION ALL
SELECT 'invalid stored value in desktop_user' AS problem FROM desktop_user WHERE
 NOT fb_valid_uuid(user_id) OR
 NOT fb_valid_text(role) OR
 NOT fb_valid_text(password_hash)
UNION ALL
SELECT 'invalid stored value in desktop_mfa_totp' FROM desktop_mfa_totp WHERE NOT fb_valid_uuid(user_id)
UNION ALL
SELECT 'invalid stored value in desktop_mfa_recovery' FROM desktop_mfa_recovery WHERE NOT fb_valid_uuid(user_id)
UNION ALL
SELECT 'invalid stored value in invoice_preferences' AS problem FROM invoice_preferences WHERE
 NOT fb_valid_uuid(company_id) OR NOT fb_valid_text(data)
UNION ALL
SELECT 'invalid stored value in invoice_email_connection' AS problem FROM invoice_email_connection WHERE
 NOT fb_valid_text(client_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(from_address) OR
 NOT fb_valid_text(status) OR
 NOT fb_valid_timestamp(updated_at) OR
 NOT fb_valid_uuid(updated_by_id)
UNION ALL
SELECT 'invalid stored value in invoice_email_delivery' AS problem FROM invoice_email_delivery WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_timestamp(completed_at) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_uuid(created_by_id) OR
 NOT fb_valid_text(document_hash) OR
 NOT fb_valid_text(error_code) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(invoice_id) OR
 NOT fb_valid_uuid(invoice_revision_id) OR
 NOT fb_valid_text(message_key) OR
 NOT fb_valid_text(mode) OR
 NOT fb_valid_text(pdf_sha256) OR
 NOT fb_valid_text(provider_draft) OR
 NOT fb_valid_text(provider_message) OR
 NOT fb_valid_text(state) OR
 NOT fb_valid_text(subject) OR
 NOT fb_valid_text(to_address)
UNION ALL
SELECT 'invalid stored value in invoice_stripe_connection' AS problem FROM invoice_stripe_connection WHERE NOT fb_valid_uuid(id) OR NOT fb_valid_uuid(company_id) OR NOT fb_valid_text(account_key) OR NOT fb_valid_text(mode) OR NOT fb_valid_text(key_last4) OR NOT fb_valid_text(status) OR NOT fb_valid_timestamp(created_at) OR NOT fb_valid_timestamp(updated_at) OR NOT fb_valid_uuid(updated_by_id)
UNION ALL
SELECT 'invalid stored value in invoice_stripe_link' AS problem FROM invoice_stripe_link WHERE NOT fb_valid_uuid(id) OR NOT fb_valid_uuid(company_id) OR NOT fb_valid_uuid(connection_id) OR NOT fb_valid_uuid(invoice_id) OR NOT fb_valid_uuid(invoice_revision_id) OR NOT fb_valid_uuid(created_by_id) OR NOT fb_valid_text(description) OR NOT fb_valid_text(currency) OR NOT fb_valid_text(state) OR NOT fb_valid_text(provider_key) OR NOT fb_valid_text(payment_url) OR NOT fb_valid_text(cursor) OR NOT fb_valid_timestamp(created_at) OR NOT fb_valid_timestamp(checked_at)
UNION ALL
SELECT 'invalid stored value in invoice_stripe_payment' AS problem FROM invoice_stripe_payment WHERE NOT fb_valid_uuid(id) OR NOT fb_valid_uuid(company_id) OR NOT fb_valid_uuid(connection_id) OR NOT fb_valid_uuid(link_id) OR NOT fb_valid_text(account_key) OR NOT fb_valid_text(mode) OR NOT fb_valid_text(session_key) OR NOT fb_valid_text(intent_key) OR NOT fb_valid_text(currency) OR NOT fb_valid_text(provider_state) OR NOT fb_valid_timestamp(observed_at) OR NOT fb_valid_uuid(receipt_id)
UNION ALL
SELECT 'invalid stored value in bankfeeds_rule_policy' AS problem FROM bankfeeds_rule_policy WHERE NOT fb_valid_uuid(company_id) OR NOT fb_valid_text(data) OR NOT fb_valid_uuid(rule_id) OR NOT fb_valid_text(source_hash) OR NOT fb_valid_text(source_key)
UNION ALL
SELECT 'invalid stored value in bankfeeds_rule_review' AS problem FROM bankfeeds_rule_review WHERE NOT fb_valid_uuid(item_id) OR NOT fb_valid_text(reason)
UNION ALL
SELECT 'invalid stored value in bankfeeds_auto_queue' AS problem FROM bankfeeds_auto_queue WHERE NOT fb_valid_text(fingerprint) OR NOT fb_valid_uuid(item_id) OR NOT fb_valid_uuid(rule_id)
UNION ALL SELECT 'sales_importinvoice:'||invoice_id FROM sales_importinvoice WHERE NOT fb_valid_uuid(company_id) OR NOT fb_valid_text(source) OR NOT fb_valid_uuid(source_id) OR NOT fb_valid_text(source_number) OR NOT fb_valid_uuid(invoice_id) OR NOT fb_valid_uuid(revision_id) OR NOT fb_valid_text(digest) OR NOT fb_valid_text(data) OR NOT fb_valid_uuid(actor_id) OR NOT fb_valid_uuid(audit_id)
UNION ALL SELECT 'sales_importpayment:'||invoice_id FROM sales_importpayment WHERE NOT fb_valid_uuid(company_id) OR NOT fb_valid_text(source) OR NOT fb_valid_uuid(source_id) OR NOT fb_valid_uuid(invoice_id) OR NOT fb_valid_uuid(receipt_id) OR NOT fb_valid_text(external_key)
UNION ALL SELECT 'sales_importseal:'||invoice_id FROM sales_importseal WHERE NOT fb_valid_uuid(invoice_id)
UNION ALL SELECT 'salestax_payment:'||id FROM salestax_payment WHERE NOT fb_valid_uuid(id) OR NOT fb_valid_uuid(company_id) OR NOT fb_valid_uuid(identity_id) OR NOT fb_valid_text(creation_digest) OR NOT fb_valid_text(review_digest) OR NOT fb_valid_uuid(agency_id) OR NOT fb_valid_text(agency_name) OR NOT fb_valid_uuid(bank_id) OR NOT fb_valid_uuid(tax_account_id) OR NOT fb_valid_uuid(expense_account_id) OR NOT fb_valid_date(date) OR NOT fb_valid_date(period_start_date) OR NOT fb_valid_date(period_end_date) OR NOT fb_valid_text(reference) OR NOT fb_valid_text(memo) OR NOT fb_valid_uuid(journal_id) OR NOT fb_valid_uuid(actor_id) OR NOT fb_valid_text(actor_name) OR NOT fb_valid_timestamp(created_at)
UNION ALL SELECT 'salestax_paymentvoid:'||id FROM salestax_paymentvoid WHERE NOT fb_valid_uuid(id) OR NOT fb_valid_uuid(payment_id) OR NOT fb_valid_uuid(journal_id) OR NOT fb_valid_date(date) OR NOT fb_valid_text(reason) OR NOT fb_valid_uuid(actor_id) OR NOT fb_valid_text(actor_name) OR NOT fb_valid_timestamp(created_at)
UNION ALL SELECT 'attachments_keywrap:'||company_id||':'||stored_name FROM attachments_keywrap WHERE NOT fb_valid_uuid(audit_id) OR NOT fb_valid_uuid(company_id) OR NOT fb_valid_text(stored_name) OR NOT fb_valid_text(wrap_nonce) OR NOT fb_valid_text(wrapped_key)
UNION ALL SELECT 'invalid stored value in payables_recurring' AS problem FROM payables_recurring WHERE NOT fb_valid_uuid(source_bill_id) OR NOT fb_valid_date(source_date) OR NOT fb_valid_uuid(id) OR NOT fb_valid_uuid(company_id) OR NOT fb_valid_text(name) OR NOT fb_valid_text(frequency) OR NOT fb_valid_date(start_date) OR NOT fb_valid_date(end_date) OR NOT fb_valid_text(data)
UNION ALL SELECT 'invalid stored value in payables_recurring_occurrence' AS problem FROM payables_recurring_occurrence WHERE NOT fb_valid_uuid(company_id) OR NOT fb_valid_uuid(template_id) OR NOT fb_valid_date(occurrence_date) OR NOT fb_valid_uuid(bill_id)
UNION ALL
SELECT 'invalid stored value in jobs_billing' AS problem FROM jobs_billing WHERE
 NOT fb_valid_uuid(id) OR NOT fb_valid_uuid(company_id) OR NOT fb_valid_uuid(entry_id) OR NOT fb_valid_uuid(invoice_id) OR NOT fb_valid_uuid(ticket_id) OR NOT fb_valid_uuid(customer_id) OR NOT fb_valid_uuid(created_by_id) OR NOT fb_valid_timestamp(created_at)
UNION ALL
SELECT 'invalid stored value in jobs_billingstep' AS problem FROM jobs_billingstep WHERE
 NOT fb_valid_uuid(id) OR NOT fb_valid_uuid(company_id) OR NOT fb_valid_uuid(billing_id) OR NOT fb_valid_uuid(invoice_id) OR NOT fb_valid_uuid(created_by_id) OR NOT fb_valid_timestamp(created_at)
UNION ALL
SELECT 'invalid stored value in jobs_ticket' AS problem FROM jobs_ticket WHERE
 NOT fb_valid_uuid(id) OR NOT fb_valid_uuid(company_id) OR NOT fb_valid_text(number) OR NOT fb_valid_uuid(request_key) OR NOT fb_valid_uuid(customer_id) OR NOT fb_valid_text(title) OR NOT fb_valid_text(description) OR NOT fb_valid_text(status) OR NOT fb_valid_date(opened_date) OR NOT fb_valid_date(due_date) OR NOT fb_valid_date(finished_date) OR NOT fb_valid_uuid(created_by_id) OR NOT fb_valid_timestamp(created_at) OR NOT fb_valid_timestamp(updated_at)
UNION ALL
SELECT 'invalid stored value in jobs_timeentry' AS problem FROM jobs_timeentry WHERE
 NOT fb_valid_uuid(id) OR NOT fb_valid_uuid(company_id) OR NOT fb_valid_uuid(request_key) OR NOT fb_valid_uuid(customer_id) OR NOT fb_valid_uuid(ticket_id) OR NOT fb_valid_uuid(item_id) OR NOT fb_valid_date(date) OR NOT fb_valid_text(notes) OR NOT fb_valid_uuid(worker_id) OR NOT fb_valid_uuid(created_by_id) OR NOT fb_valid_timestamp(created_at) OR NOT fb_valid_timestamp(updated_at);
