-- Stripe metadata only; API keys stay in the OS-sealed plugin vault. Provider
-- observations never post accounting work; the receipt capability remains explicit.
CREATE TABLE invoice_stripe_connection (
 id TEXT PRIMARY KEY NOT NULL, company_id TEXT NOT NULL REFERENCES ledger_company(id),
 account_key TEXT NOT NULL, mode TEXT NOT NULL, key_last4 TEXT NOT NULL, status TEXT NOT NULL,
 revision INTEGER NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
 updated_by_id TEXT NOT NULL REFERENCES fb_user(id),
 UNIQUE(company_id,account_key,mode), CHECK(typeof(revision)='integer' AND revision BETWEEN 1 AND 9223372036854775807 AND mode IN ('TEST','LIVE') AND status IN ('CONNECTED','DISCONNECTED') AND (length(account_key) BETWEEN 6 AND 256 AND substr(account_key,1,5)='acct_' AND account_key NOT GLOB '*[^a-zA-Z0-9_]*') AND length(key_last4)=4 AND key_last4 NOT GLOB '*[^a-zA-Z0-9_]*' AND updated_at>=created_at)
);
CREATE UNIQUE INDEX invoice_stripe_current ON invoice_stripe_connection(company_id,mode) WHERE status='CONNECTED';
CREATE TABLE invoice_stripe_link (
 id TEXT PRIMARY KEY NOT NULL, company_id TEXT NOT NULL REFERENCES ledger_company(id),
 connection_id TEXT NOT NULL REFERENCES invoice_stripe_connection(id), invoice_id TEXT NOT NULL REFERENCES sales_invoice(id),
 invoice_revision_id TEXT NOT NULL REFERENCES sales_invoicerevision(id), created_by_id TEXT NOT NULL REFERENCES fb_user(id),
 description TEXT NOT NULL, amount INTEGER NOT NULL, currency TEXT NOT NULL, state TEXT NOT NULL,
 provider_key TEXT NOT NULL DEFAULT '', payment_url TEXT NOT NULL DEFAULT '', cursor TEXT NOT NULL DEFAULT '',
 created_at TEXT NOT NULL, checked_at TEXT, revision INTEGER NOT NULL,
 CHECK(typeof(revision)='integer' AND revision BETWEEN 1 AND 9223372036854775807 AND typeof(amount)='integer' AND amount BETWEEN 50 AND 99999999 AND currency='USD' AND length(description) BETWEEN 1 AND 500 AND state IN ('CREATING','UNKNOWN','REFUSED','READY','INACTIVE','REACTIVATED') AND (provider_key='' OR (length(provider_key) BETWEEN 7 AND 256 AND substr(provider_key,1,6)='plink_' AND provider_key NOT GLOB '*[^a-zA-Z0-9_]*')) AND (cursor='' OR (length(cursor) BETWEEN 4 AND 256 AND substr(cursor,1,3)='cs_' AND cursor NOT GLOB '*[^a-zA-Z0-9_]*')) AND ((state IN ('READY','INACTIVE','REACTIVATED') AND provider_key!='' AND payment_url LIKE 'https://buy.stripe.com/%' AND length(payment_url)<=2048) OR (state IN ('CREATING','UNKNOWN','REFUSED') AND provider_key='' AND payment_url='' AND cursor='')) AND (checked_at IS NULL OR checked_at>=created_at))
);
CREATE UNIQUE INDEX invoice_stripe_open_request ON invoice_stripe_link(connection_id,invoice_id) WHERE state IN ('CREATING','UNKNOWN','READY');
CREATE UNIQUE INDEX invoice_stripe_remote_link ON invoice_stripe_link(connection_id,provider_key) WHERE provider_key!='';
CREATE TABLE invoice_stripe_payment (
 id TEXT PRIMARY KEY NOT NULL, company_id TEXT NOT NULL REFERENCES ledger_company(id),
 connection_id TEXT NOT NULL REFERENCES invoice_stripe_connection(id), link_id TEXT NOT NULL REFERENCES invoice_stripe_link(id),
 account_key TEXT NOT NULL, mode TEXT NOT NULL, session_key TEXT NOT NULL, intent_key TEXT NOT NULL,
 amount INTEGER NOT NULL, currency TEXT NOT NULL, received INTEGER NOT NULL, refunded INTEGER NOT NULL,
 disputed INTEGER NOT NULL, provider_state TEXT NOT NULL, provider_created_epoch INTEGER NOT NULL,
 observed_at TEXT NOT NULL, receipt_id TEXT REFERENCES sales_receipt(id), revision INTEGER NOT NULL,
 UNIQUE(account_key,mode,session_key), CHECK(typeof(revision)='integer' AND revision BETWEEN 1 AND 9223372036854775807 AND typeof(amount)='integer' AND amount BETWEEN 0 AND 99999999 AND typeof(received)='integer' AND received BETWEEN 0 AND 99999999 AND typeof(refunded)='integer' AND refunded BETWEEN 0 AND 99999999 AND typeof(provider_created_epoch)='integer' AND provider_created_epoch BETWEEN 1 AND 253402300799 AND typeof(disputed)='integer' AND disputed BETWEEN 0 AND 1 AND (length(account_key) BETWEEN 6 AND 256 AND substr(account_key,1,5)='acct_' AND account_key NOT GLOB '*[^a-zA-Z0-9_]*') AND mode IN ('TEST','LIVE') AND (length(session_key) BETWEEN 4 AND 256 AND substr(session_key,1,3)='cs_' AND session_key NOT GLOB '*[^a-zA-Z0-9_]*') AND (intent_key='' OR (length(intent_key) BETWEEN 4 AND 256 AND substr(intent_key,1,3)='pi_' AND intent_key NOT GLOB '*[^a-zA-Z0-9_]*')) AND length(currency)=3 AND currency NOT GLOB '*[^A-Z]*' AND provider_state IN ('PENDING','READY','FAILED','MISMATCH','REFUNDED','DISPUTED') AND (receipt_id IS NULL OR mode='LIVE') AND (provider_state!='READY' OR (amount>0 AND received=amount AND refunded=0 AND disputed=0 AND intent_key!='')))
);
CREATE UNIQUE INDEX invoice_stripe_unique_intent ON invoice_stripe_payment(account_key,mode,intent_key) WHERE intent_key!='';
CREATE TRIGGER invoice_stripe_connection_insert BEFORE INSERT ON invoice_stripe_connection BEGIN
 SELECT RAISE(ABORT,'Stripe: History cannot be replaced') WHERE NEW.revision!=1 OR EXISTS(SELECT 1 FROM invoice_stripe_connection WHERE id=NEW.id);
END;
CREATE TRIGGER invoice_stripe_connection_update BEFORE UPDATE ON invoice_stripe_connection BEGIN
 SELECT RAISE(ABORT,'Stripe: Identity or revision changed') WHERE NEW.revision!=OLD.revision+1 OR NEW.id IS NOT OLD.id OR NEW.company_id IS NOT OLD.company_id OR NEW.account_key IS NOT OLD.account_key OR NEW.mode IS NOT OLD.mode OR NEW.created_at IS NOT OLD.created_at;
 SELECT RAISE(ABORT,'Stripe: Deactivate existing links before disconnecting') WHERE NEW.status='DISCONNECTED' AND EXISTS(SELECT 1 FROM invoice_stripe_link WHERE connection_id=OLD.id AND state IN ('CREATING','UNKNOWN','READY','REACTIVATED'));
END;
CREATE TRIGGER invoice_stripe_connection_delete BEFORE DELETE ON invoice_stripe_connection BEGIN SELECT RAISE(ABORT,'Stripe: History is retained'); END;
CREATE TRIGGER invoice_stripe_connection_relation_insert AFTER INSERT ON invoice_stripe_connection BEGIN SELECT RAISE(ABORT,'Stripe: Wrong company or source') WHERE EXISTS(SELECT 1 FROM invoice_stripe_connection WHERE id=NEW.id AND NOT (1)); END;
CREATE TRIGGER fb_valid_invoice_stripe_connection_insert BEFORE INSERT ON invoice_stripe_connection BEGIN SELECT RAISE(ABORT,'fb_valid: Invalid stored value') WHERE NOT fb_valid_uuid(NEW.id) OR NOT fb_valid_uuid(NEW.company_id) OR NOT fb_valid_text(NEW.account_key) OR NOT fb_valid_text(NEW.mode) OR NOT fb_valid_text(NEW.key_last4) OR NOT fb_valid_text(NEW.status) OR NOT fb_valid_timestamp(NEW.created_at) OR NOT fb_valid_timestamp(NEW.updated_at) OR NOT fb_valid_uuid(NEW.updated_by_id); END;
CREATE TRIGGER invoice_stripe_connection_relation_update AFTER UPDATE ON invoice_stripe_connection BEGIN SELECT RAISE(ABORT,'Stripe: Wrong company or source') WHERE EXISTS(SELECT 1 FROM invoice_stripe_connection WHERE id=NEW.id AND NOT (1)); END;
CREATE TRIGGER fb_valid_invoice_stripe_connection_update BEFORE UPDATE ON invoice_stripe_connection BEGIN SELECT RAISE(ABORT,'fb_valid: Invalid stored value') WHERE NOT fb_valid_uuid(NEW.id) OR NOT fb_valid_uuid(NEW.company_id) OR NOT fb_valid_text(NEW.account_key) OR NOT fb_valid_text(NEW.mode) OR NOT fb_valid_text(NEW.key_last4) OR NOT fb_valid_text(NEW.status) OR NOT fb_valid_timestamp(NEW.created_at) OR NOT fb_valid_timestamp(NEW.updated_at) OR NOT fb_valid_uuid(NEW.updated_by_id); END;
CREATE TRIGGER invoice_stripe_link_insert BEFORE INSERT ON invoice_stripe_link BEGIN
 SELECT RAISE(ABORT,'Stripe: History cannot be replaced') WHERE NEW.revision!=1 OR EXISTS(SELECT 1 FROM invoice_stripe_link WHERE id=NEW.id);
 SELECT RAISE(ABORT,'Stripe: Invalid invoice request') WHERE NEW.state!='CREATING' OR NOT EXISTS(SELECT 1 FROM invoice_stripe_connection c JOIN sales_invoice i ON i.company_id=c.company_id JOIN sales_invoicerevision r ON r.invoice_id=i.id AND r.number=i.revision WHERE c.id=NEW.connection_id AND c.company_id=NEW.company_id AND c.status='CONNECTED' AND i.id=NEW.invoice_id AND r.id=NEW.invoice_revision_id AND r.state='OPEN');
END;
CREATE TRIGGER invoice_stripe_link_update BEFORE UPDATE ON invoice_stripe_link BEGIN
 SELECT RAISE(ABORT,'Stripe: Identity or revision changed') WHERE NEW.revision!=OLD.revision+1 OR NEW.id IS NOT OLD.id OR NEW.company_id IS NOT OLD.company_id OR NEW.connection_id IS NOT OLD.connection_id OR NEW.invoice_id IS NOT OLD.invoice_id OR NEW.invoice_revision_id IS NOT OLD.invoice_revision_id OR NEW.created_by_id IS NOT OLD.created_by_id OR NEW.description IS NOT OLD.description OR NEW.amount IS NOT OLD.amount OR NEW.currency IS NOT OLD.currency OR NEW.created_at IS NOT OLD.created_at;
 SELECT RAISE(ABORT,'Stripe: Invalid link transition') WHERE NOT ((OLD.state='CREATING' AND NEW.state IN ('READY','INACTIVE','UNKNOWN','REFUSED')) OR (OLD.state='UNKNOWN' AND NEW.state IN ('READY','INACTIVE','REFUSED','UNKNOWN')) OR (OLD.state='READY' AND NEW.state IN ('READY','INACTIVE','REACTIVATED')) OR (OLD.state IN ('INACTIVE','REACTIVATED') AND NEW.state IN ('INACTIVE','REACTIVATED'))) OR (OLD.provider_key!='' AND (NEW.provider_key IS NOT OLD.provider_key OR NEW.payment_url IS NOT OLD.payment_url));
END;
CREATE TRIGGER invoice_stripe_link_delete BEFORE DELETE ON invoice_stripe_link BEGIN SELECT RAISE(ABORT,'Stripe: History is retained'); END;
CREATE TRIGGER invoice_stripe_link_relation_insert AFTER INSERT ON invoice_stripe_link BEGIN SELECT RAISE(ABORT,'Stripe: Wrong company or source') WHERE EXISTS(SELECT 1 FROM invoice_stripe_link WHERE id=NEW.id AND NOT (EXISTS(SELECT 1 FROM invoice_stripe_connection c JOIN sales_invoice i ON i.company_id=c.company_id JOIN sales_invoicerevision r ON r.invoice_id=i.id WHERE c.id=invoice_stripe_link.connection_id AND c.company_id=invoice_stripe_link.company_id AND i.id=invoice_stripe_link.invoice_id AND r.id=invoice_stripe_link.invoice_revision_id))); END;
CREATE TRIGGER fb_valid_invoice_stripe_link_insert BEFORE INSERT ON invoice_stripe_link BEGIN SELECT RAISE(ABORT,'fb_valid: Invalid stored value') WHERE NOT fb_valid_uuid(NEW.id) OR NOT fb_valid_uuid(NEW.company_id) OR NOT fb_valid_uuid(NEW.connection_id) OR NOT fb_valid_uuid(NEW.invoice_id) OR NOT fb_valid_uuid(NEW.invoice_revision_id) OR NOT fb_valid_uuid(NEW.created_by_id) OR NOT fb_valid_text(NEW.description) OR NOT fb_valid_text(NEW.currency) OR NOT fb_valid_text(NEW.state) OR NOT fb_valid_text(NEW.provider_key) OR NOT fb_valid_text(NEW.payment_url) OR NOT fb_valid_text(NEW.cursor) OR NOT fb_valid_timestamp(NEW.created_at) OR NOT fb_valid_timestamp(NEW.checked_at); END;
CREATE TRIGGER invoice_stripe_link_relation_update AFTER UPDATE ON invoice_stripe_link BEGIN SELECT RAISE(ABORT,'Stripe: Wrong company or source') WHERE EXISTS(SELECT 1 FROM invoice_stripe_link WHERE id=NEW.id AND NOT (EXISTS(SELECT 1 FROM invoice_stripe_connection c JOIN sales_invoice i ON i.company_id=c.company_id JOIN sales_invoicerevision r ON r.invoice_id=i.id WHERE c.id=invoice_stripe_link.connection_id AND c.company_id=invoice_stripe_link.company_id AND i.id=invoice_stripe_link.invoice_id AND r.id=invoice_stripe_link.invoice_revision_id))); END;
CREATE TRIGGER fb_valid_invoice_stripe_link_update BEFORE UPDATE ON invoice_stripe_link BEGIN SELECT RAISE(ABORT,'fb_valid: Invalid stored value') WHERE NOT fb_valid_uuid(NEW.id) OR NOT fb_valid_uuid(NEW.company_id) OR NOT fb_valid_uuid(NEW.connection_id) OR NOT fb_valid_uuid(NEW.invoice_id) OR NOT fb_valid_uuid(NEW.invoice_revision_id) OR NOT fb_valid_uuid(NEW.created_by_id) OR NOT fb_valid_text(NEW.description) OR NOT fb_valid_text(NEW.currency) OR NOT fb_valid_text(NEW.state) OR NOT fb_valid_text(NEW.provider_key) OR NOT fb_valid_text(NEW.payment_url) OR NOT fb_valid_text(NEW.cursor) OR NOT fb_valid_timestamp(NEW.created_at) OR NOT fb_valid_timestamp(NEW.checked_at); END;
CREATE TRIGGER invoice_stripe_payment_insert BEFORE INSERT ON invoice_stripe_payment BEGIN
 SELECT RAISE(ABORT,'Stripe: History cannot be replaced') WHERE NEW.revision!=1 OR EXISTS(SELECT 1 FROM invoice_stripe_payment WHERE id=NEW.id);
 SELECT RAISE(ABORT,'Stripe: A payment must be reviewed before posting') WHERE NEW.receipt_id IS NOT NULL;
END;
CREATE TRIGGER invoice_stripe_payment_update BEFORE UPDATE ON invoice_stripe_payment BEGIN
 SELECT RAISE(ABORT,'Stripe: Identity or revision changed') WHERE NEW.revision!=OLD.revision+1 OR NEW.id IS NOT OLD.id OR NEW.company_id IS NOT OLD.company_id OR NEW.connection_id IS NOT OLD.connection_id OR NEW.link_id IS NOT OLD.link_id OR NEW.account_key IS NOT OLD.account_key OR NEW.mode IS NOT OLD.mode OR NEW.session_key IS NOT OLD.session_key OR NEW.amount IS NOT OLD.amount OR NEW.currency IS NOT OLD.currency OR NEW.provider_created_epoch IS NOT OLD.provider_created_epoch;
 SELECT RAISE(ABORT,'Stripe: Payment identity and posting are permanent') WHERE (OLD.intent_key!='' AND NEW.intent_key IS NOT OLD.intent_key) OR (OLD.receipt_id IS NOT NULL AND NEW.receipt_id IS NOT OLD.receipt_id);
 SELECT RAISE(ABORT,'Stripe: Only reviewed live payments can post') WHERE OLD.receipt_id IS NULL AND NEW.receipt_id IS NOT NULL AND (NEW.mode!='LIVE' OR NEW.provider_state!='READY' OR NOT EXISTS(SELECT 1 FROM invoice_stripe_link WHERE id=NEW.link_id AND state='INACTIVE'));
END;
CREATE TRIGGER invoice_stripe_payment_delete BEFORE DELETE ON invoice_stripe_payment BEGIN SELECT RAISE(ABORT,'Stripe: History is retained'); END;
CREATE TRIGGER invoice_stripe_payment_relation_insert AFTER INSERT ON invoice_stripe_payment BEGIN SELECT RAISE(ABORT,'Stripe: Wrong company or source') WHERE EXISTS(SELECT 1 FROM invoice_stripe_payment WHERE id=NEW.id AND NOT (EXISTS(SELECT 1 FROM invoice_stripe_connection c JOIN invoice_stripe_link l ON l.connection_id=c.id WHERE c.id=invoice_stripe_payment.connection_id AND l.id=invoice_stripe_payment.link_id AND c.company_id=invoice_stripe_payment.company_id AND l.company_id=c.company_id AND c.account_key=invoice_stripe_payment.account_key AND c.mode=invoice_stripe_payment.mode) AND (receipt_id IS NULL OR EXISTS(SELECT 1 FROM sales_receipt r JOIN documents_documentidentity d ON d.id=r.identity_id JOIN sales_receiptallocation a ON a.receipt_id=r.id JOIN invoice_stripe_link l ON l.id=invoice_stripe_payment.link_id WHERE r.id=invoice_stripe_payment.receipt_id AND r.company_id=invoice_stripe_payment.company_id AND r.amount=invoice_stripe_payment.amount AND r.kind='RCP' AND d.request_key=invoice_stripe_payment.id AND a.invoice_id=l.invoice_id AND a.amount=invoice_stripe_payment.amount)))); END;
CREATE TRIGGER fb_valid_invoice_stripe_payment_insert BEFORE INSERT ON invoice_stripe_payment BEGIN SELECT RAISE(ABORT,'fb_valid: Invalid stored value') WHERE NOT fb_valid_uuid(NEW.id) OR NOT fb_valid_uuid(NEW.company_id) OR NOT fb_valid_uuid(NEW.connection_id) OR NOT fb_valid_uuid(NEW.link_id) OR NOT fb_valid_text(NEW.account_key) OR NOT fb_valid_text(NEW.mode) OR NOT fb_valid_text(NEW.session_key) OR NOT fb_valid_text(NEW.intent_key) OR NOT fb_valid_text(NEW.currency) OR NOT fb_valid_text(NEW.provider_state) OR NOT fb_valid_timestamp(NEW.observed_at) OR NOT fb_valid_uuid(NEW.receipt_id); END;
CREATE TRIGGER invoice_stripe_payment_relation_update AFTER UPDATE ON invoice_stripe_payment BEGIN SELECT RAISE(ABORT,'Stripe: Wrong company or source') WHERE EXISTS(SELECT 1 FROM invoice_stripe_payment WHERE id=NEW.id AND NOT (EXISTS(SELECT 1 FROM invoice_stripe_connection c JOIN invoice_stripe_link l ON l.connection_id=c.id WHERE c.id=invoice_stripe_payment.connection_id AND l.id=invoice_stripe_payment.link_id AND c.company_id=invoice_stripe_payment.company_id AND l.company_id=c.company_id AND c.account_key=invoice_stripe_payment.account_key AND c.mode=invoice_stripe_payment.mode) AND (receipt_id IS NULL OR EXISTS(SELECT 1 FROM sales_receipt r JOIN documents_documentidentity d ON d.id=r.identity_id JOIN sales_receiptallocation a ON a.receipt_id=r.id JOIN invoice_stripe_link l ON l.id=invoice_stripe_payment.link_id WHERE r.id=invoice_stripe_payment.receipt_id AND r.company_id=invoice_stripe_payment.company_id AND r.amount=invoice_stripe_payment.amount AND r.kind='RCP' AND d.request_key=invoice_stripe_payment.id AND a.invoice_id=l.invoice_id AND a.amount=invoice_stripe_payment.amount)))); END;
CREATE TRIGGER fb_valid_invoice_stripe_payment_update BEFORE UPDATE ON invoice_stripe_payment BEGIN SELECT RAISE(ABORT,'fb_valid: Invalid stored value') WHERE NOT fb_valid_uuid(NEW.id) OR NOT fb_valid_uuid(NEW.company_id) OR NOT fb_valid_uuid(NEW.connection_id) OR NOT fb_valid_uuid(NEW.link_id) OR NOT fb_valid_text(NEW.account_key) OR NOT fb_valid_text(NEW.mode) OR NOT fb_valid_text(NEW.session_key) OR NOT fb_valid_text(NEW.intent_key) OR NOT fb_valid_text(NEW.currency) OR NOT fb_valid_text(NEW.provider_state) OR NOT fb_valid_timestamp(NEW.observed_at) OR NOT fb_valid_uuid(NEW.receipt_id); END;
CREATE TRIGGER stripe_invoice_change BEFORE UPDATE OF revision ON sales_invoice BEGIN SELECT RAISE(ABORT,'Stripe: Deactivate the payment link and review pending payments before changing this invoice balance') WHERE EXISTS(SELECT 1 FROM invoice_stripe_link WHERE invoice_id=NEW.id AND connection_id IN (SELECT id FROM invoice_stripe_connection WHERE mode='LIVE') AND state IN ('CREATING','UNKNOWN','READY','REACTIVATED')) OR EXISTS(SELECT 1 FROM invoice_stripe_payment p JOIN invoice_stripe_link l ON l.id=p.link_id WHERE l.invoice_id=NEW.id AND p.mode='LIVE' AND p.intent_key!='' AND p.receipt_id IS NULL AND p.provider_state IN ('PENDING','READY')); END;
CREATE TRIGGER stripe_receipt_allocation BEFORE INSERT ON sales_receiptallocation BEGIN SELECT RAISE(ABORT,'Stripe: Deactivate the payment link and review pending payments before changing this invoice balance') WHERE EXISTS(SELECT 1 FROM invoice_stripe_link WHERE invoice_id=NEW.invoice_id AND connection_id IN (SELECT id FROM invoice_stripe_connection WHERE mode='LIVE') AND state IN ('CREATING','UNKNOWN','READY','REACTIVATED')) OR EXISTS(SELECT 1 FROM invoice_stripe_payment p JOIN invoice_stripe_link l ON l.id=p.link_id WHERE l.invoice_id=NEW.invoice_id AND p.mode='LIVE' AND p.intent_key!='' AND p.receipt_id IS NULL AND p.provider_state IN ('PENDING','READY') AND NOT EXISTS(SELECT 1 FROM sales_receipt r JOIN documents_documentidentity d ON d.id=r.identity_id WHERE r.id=NEW.receipt_id AND d.request_key=p.id AND p.mode='LIVE' AND p.provider_state='READY' AND p.amount=NEW.amount)); END;
CREATE TRIGGER stripe_credit_allocation BEFORE INSERT ON credits_creditapplicationline BEGIN SELECT RAISE(ABORT,'Stripe: Deactivate the payment link and review pending payments before changing this invoice balance') WHERE EXISTS(SELECT 1 FROM invoice_stripe_link WHERE invoice_id=NEW.invoice_id AND connection_id IN (SELECT id FROM invoice_stripe_connection WHERE mode='LIVE') AND state IN ('CREATING','UNKNOWN','READY','REACTIVATED')) OR EXISTS(SELECT 1 FROM invoice_stripe_payment p JOIN invoice_stripe_link l ON l.id=p.link_id WHERE l.invoice_id=NEW.invoice_id AND p.mode='LIVE' AND p.intent_key!='' AND p.receipt_id IS NULL AND p.provider_state IN ('PENDING','READY')); END;
CREATE TRIGGER stripe_deposit_allocation BEFORE INSERT ON prepayments_depositapplication BEGIN SELECT RAISE(ABORT,'Stripe: Deactivate the payment link and review pending payments before changing this invoice balance') WHERE EXISTS(SELECT 1 FROM invoice_stripe_link WHERE invoice_id=NEW.invoice_id AND connection_id IN (SELECT id FROM invoice_stripe_connection WHERE mode='LIVE') AND state IN ('CREATING','UNKNOWN','READY','REACTIVATED')) OR EXISTS(SELECT 1 FROM invoice_stripe_payment p JOIN invoice_stripe_link l ON l.id=p.link_id WHERE l.invoice_id=NEW.invoice_id AND p.mode='LIVE' AND p.intent_key!='' AND p.receipt_id IS NULL AND p.provider_state IN ('PENDING','READY')); END;
CREATE TRIGGER stripe_receipt_reversal BEFORE INSERT ON sales_receiptreversal BEGIN SELECT RAISE(ABORT,'Stripe: Deactivate the payment link before changing this invoice balance') WHERE EXISTS(SELECT 1 FROM invoice_stripe_link WHERE invoice_id IN (SELECT invoice_id FROM sales_receiptallocation WHERE receipt_id=NEW.receipt_id) AND connection_id IN (SELECT id FROM invoice_stripe_connection WHERE mode='LIVE') AND state IN ('CREATING','UNKNOWN','READY','REACTIVATED')); END;
CREATE TRIGGER stripe_credit_unapplication BEFORE INSERT ON credits_creditunapplication BEGIN SELECT RAISE(ABORT,'Stripe: Deactivate the payment link before changing this invoice balance') WHERE EXISTS(SELECT 1 FROM invoice_stripe_link WHERE invoice_id IN (SELECT invoice_id FROM credits_creditapplicationline WHERE application_id=NEW.application_id) AND connection_id IN (SELECT id FROM invoice_stripe_connection WHERE mode='LIVE') AND state IN ('CREATING','UNKNOWN','READY','REACTIVATED')); END;
CREATE TRIGGER stripe_deposit_unapplication BEFORE INSERT ON prepayments_depositunapplication BEGIN SELECT RAISE(ABORT,'Stripe: Deactivate the payment link before changing this invoice balance') WHERE EXISTS(SELECT 1 FROM invoice_stripe_link WHERE invoice_id IN (SELECT invoice_id FROM prepayments_depositapplication WHERE id=NEW.application_id) AND connection_id IN (SELECT id FROM invoice_stripe_connection WHERE mode='LIVE') AND state IN ('CREATING','UNKNOWN','READY','REACTIVATED')); END;
CREATE VIEW invoice_stripe_invalid AS
SELECT id FROM invoice_stripe_connection WHERE NOT (typeof(revision)='integer' AND revision BETWEEN 1 AND 9223372036854775807 AND mode IN ('TEST','LIVE') AND status IN ('CONNECTED','DISCONNECTED') AND (length(account_key) BETWEEN 6 AND 256 AND substr(account_key,1,5)='acct_' AND account_key NOT GLOB '*[^a-zA-Z0-9_]*') AND length(key_last4)=4 AND key_last4 NOT GLOB '*[^a-zA-Z0-9_]*' AND updated_at>=created_at) OR NOT (1)
UNION ALL
SELECT id FROM invoice_stripe_link WHERE NOT (typeof(revision)='integer' AND revision BETWEEN 1 AND 9223372036854775807 AND typeof(amount)='integer' AND amount BETWEEN 50 AND 99999999 AND currency='USD' AND length(description) BETWEEN 1 AND 500 AND state IN ('CREATING','UNKNOWN','REFUSED','READY','INACTIVE','REACTIVATED') AND (provider_key='' OR (length(provider_key) BETWEEN 7 AND 256 AND substr(provider_key,1,6)='plink_' AND provider_key NOT GLOB '*[^a-zA-Z0-9_]*')) AND (cursor='' OR (length(cursor) BETWEEN 4 AND 256 AND substr(cursor,1,3)='cs_' AND cursor NOT GLOB '*[^a-zA-Z0-9_]*')) AND ((state IN ('READY','INACTIVE','REACTIVATED') AND provider_key!='' AND payment_url LIKE 'https://buy.stripe.com/%' AND length(payment_url)<=2048) OR (state IN ('CREATING','UNKNOWN','REFUSED') AND provider_key='' AND payment_url='' AND cursor='')) AND (checked_at IS NULL OR checked_at>=created_at)) OR NOT (EXISTS(SELECT 1 FROM invoice_stripe_connection c JOIN sales_invoice i ON i.company_id=c.company_id JOIN sales_invoicerevision r ON r.invoice_id=i.id WHERE c.id=invoice_stripe_link.connection_id AND c.company_id=invoice_stripe_link.company_id AND i.id=invoice_stripe_link.invoice_id AND r.id=invoice_stripe_link.invoice_revision_id))
UNION ALL
SELECT id FROM invoice_stripe_payment WHERE NOT (typeof(revision)='integer' AND revision BETWEEN 1 AND 9223372036854775807 AND typeof(amount)='integer' AND amount BETWEEN 0 AND 99999999 AND typeof(received)='integer' AND received BETWEEN 0 AND 99999999 AND typeof(refunded)='integer' AND refunded BETWEEN 0 AND 99999999 AND typeof(provider_created_epoch)='integer' AND provider_created_epoch BETWEEN 1 AND 253402300799 AND typeof(disputed)='integer' AND disputed BETWEEN 0 AND 1 AND (length(account_key) BETWEEN 6 AND 256 AND substr(account_key,1,5)='acct_' AND account_key NOT GLOB '*[^a-zA-Z0-9_]*') AND mode IN ('TEST','LIVE') AND (length(session_key) BETWEEN 4 AND 256 AND substr(session_key,1,3)='cs_' AND session_key NOT GLOB '*[^a-zA-Z0-9_]*') AND (intent_key='' OR (length(intent_key) BETWEEN 4 AND 256 AND substr(intent_key,1,3)='pi_' AND intent_key NOT GLOB '*[^a-zA-Z0-9_]*')) AND length(currency)=3 AND currency NOT GLOB '*[^A-Z]*' AND provider_state IN ('PENDING','READY','FAILED','MISMATCH','REFUNDED','DISPUTED') AND (receipt_id IS NULL OR mode='LIVE') AND (provider_state!='READY' OR (amount>0 AND received=amount AND refunded=0 AND disputed=0 AND intent_key!=''))) OR NOT (EXISTS(SELECT 1 FROM invoice_stripe_connection c JOIN invoice_stripe_link l ON l.connection_id=c.id WHERE c.id=invoice_stripe_payment.connection_id AND l.id=invoice_stripe_payment.link_id AND c.company_id=invoice_stripe_payment.company_id AND l.company_id=c.company_id AND c.account_key=invoice_stripe_payment.account_key AND c.mode=invoice_stripe_payment.mode) AND (receipt_id IS NULL OR EXISTS(SELECT 1 FROM sales_receipt r JOIN documents_documentidentity d ON d.id=r.identity_id JOIN sales_receiptallocation a ON a.receipt_id=r.id JOIN invoice_stripe_link l ON l.id=invoice_stripe_payment.link_id WHERE r.id=invoice_stripe_payment.receipt_id AND r.company_id=invoice_stripe_payment.company_id AND r.amount=invoice_stripe_payment.amount AND r.kind='RCP' AND d.request_key=invoice_stripe_payment.id AND a.invoice_id=l.invoice_id AND a.amount=invoice_stripe_payment.amount)));
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
SELECT 'invalid stored value in invoice_stripe_payment' AS problem FROM invoice_stripe_payment WHERE NOT fb_valid_uuid(id) OR NOT fb_valid_uuid(company_id) OR NOT fb_valid_uuid(connection_id) OR NOT fb_valid_uuid(link_id) OR NOT fb_valid_text(account_key) OR NOT fb_valid_text(mode) OR NOT fb_valid_text(session_key) OR NOT fb_valid_text(intent_key) OR NOT fb_valid_text(currency) OR NOT fb_valid_text(provider_state) OR NOT fb_valid_timestamp(observed_at) OR NOT fb_valid_uuid(receipt_id);
