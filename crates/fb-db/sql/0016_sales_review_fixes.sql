-- Eligibility is checked when a receipt is created, never against today's
-- mutable configuration when historical receipts are reopened or reversed.
CREATE TRIGGER fb_sales_receipt_destination BEFORE INSERT ON sales_receipt
BEGIN
 SELECT RAISE(ABORT,'receipt_destination: Choose a configured receipt destination')
 WHERE NOT EXISTS(SELECT 1 FROM documents_accountingsetup s
   JOIN ledger_account a ON a.id=NEW.destination_account_id
   WHERE s.company_id=NEW.company_id AND a.company_id=s.company_id
     AND a.kind='ASSET' AND a.active=1 AND a.postable=1
     AND ((NEW.destination='BANK' AND EXISTS(SELECT 1 FROM documents_designatedbank b
           WHERE b.setup_id=s.id AND b.account_id=a.id))
       OR (NEW.destination='UNDEPOSITED' AND s.undeposited_id=a.id)));
END;

-- Provenance is persisted in immutable line JSON so existing rows/keys and
-- historical applied migration prefixes remain byte-for-byte unchanged.
-- Pre-upgrade lines have no origin; new ineligible picks need explicit proof.
CREATE VIEW sales_origin_violations AS
 SELECT l.id FROM sales_invoiceline l JOIN sales_invoicerevision r ON r.id=l.revision_id
 WHERE r.state='DRAFT' AND (
   (json_extract(l.data,'$.origin_position') IS NOT NULL AND (SELECT count(*) FROM sales_invoiceline sibling WHERE sibling.revision_id=r.id AND json_extract(sibling.data,'$.origin_position')=json_extract(l.data,'$.origin_position'))>1) OR
   (json_extract(l.data,'$.origin_position') IS NULL) <>
     (json_extract(l.data,'$.origin_revision_id') IS NULL)
   OR (json_extract(l.data,'$.origin_position') IS NOT NULL AND (
     json_type(l.data,'$.origin_position') IS NOT 'integer'
     OR json_type(l.data,'$.origin_revision_id') IS NOT 'text'
     OR NOT EXISTS(SELECT 1 FROM sales_invoicerevision p JOIN sales_invoiceline prior ON prior.revision_id=p.id
       WHERE p.id=json_extract(l.data,'$.origin_revision_id') AND p.invoice_id=r.invoice_id
         AND p.number=r.number-1 AND prior.position=json_extract(l.data,'$.origin_position')))))
 UNION ALL
 SELECT l.id FROM sales_invoiceline l JOIN sales_invoicerevision r ON r.id=l.revision_id
 WHERE r.state IN('OPEN','VOID') AND NOT EXISTS(
   SELECT 1 FROM sales_invoicerevision p JOIN sales_invoiceline prior ON prior.revision_id=p.id
   WHERE p.invoice_id=r.invoice_id AND p.number=r.number-1
     AND prior.position=l.position AND prior.data=l.data);
CREATE TRIGGER fb_sales_line_origin BEFORE INSERT ON sales_invoiceline
BEGIN
 SELECT RAISE(ABORT,'invoice_line_origin: Invalid or duplicated retained line identity')
 WHERE EXISTS(SELECT 1 FROM sales_invoicerevision r WHERE r.id=NEW.revision_id AND r.state='DRAFT'
   AND (((json_extract(NEW.data,'$.origin_position') IS NULL) <>
          (json_extract(NEW.data,'$.origin_revision_id') IS NULL))
     OR (json_extract(NEW.data,'$.origin_position') IS NOT NULL AND (
       json_type(NEW.data,'$.origin_position') IS NOT 'integer'
       OR json_type(NEW.data,'$.origin_revision_id') IS NOT 'text'
       OR NOT EXISTS(SELECT 1 FROM sales_invoicerevision p JOIN sales_invoiceline prior ON prior.revision_id=p.id
         WHERE p.id=json_extract(NEW.data,'$.origin_revision_id') AND p.invoice_id=r.invoice_id
           AND p.number=r.number-1 AND prior.position=json_extract(NEW.data,'$.origin_position'))
       OR EXISTS(SELECT 1 FROM sales_invoiceline l WHERE l.revision_id=r.id
         AND json_extract(l.data,'$.origin_position')=json_extract(NEW.data,'$.origin_position'))))));
END;
DROP TRIGGER fb_sales_invoice_line_eligible;
CREATE TRIGGER fb_sales_invoice_line_eligible BEFORE INSERT ON sales_invoiceline
WHEN NOT EXISTS(SELECT 1 FROM sales_invoiceline WHERE id=NEW.id OR (revision_id=NEW.revision_id AND position=NEW.position))
BEGIN
 SELECT RAISE(ABORT,'invoice_line_scope: New lines require eligible income accounts and items')
 WHERE NOT EXISTS(SELECT 1 FROM sales_invoicerevision r WHERE r.id=NEW.revision_id
 AND (NEW.income_account_id IS NULL OR EXISTS(SELECT 1 FROM ledger_account a
      WHERE a.id=NEW.income_account_id AND a.kind='INCOME' AND a.active=1 AND a.postable=1)
      OR (r.state<>'OPEN' AND EXISTS(SELECT 1 FROM sales_invoicerevision p JOIN sales_invoiceline l ON l.revision_id=p.id
          WHERE p.invoice_id=r.invoice_id AND p.number=r.number-1
            AND l.position=CASE WHEN r.state='DRAFT' THEN json_extract(NEW.data,'$.origin_position') ELSE NEW.position END
            AND (r.state<>'DRAFT' OR p.id=json_extract(NEW.data,'$.origin_revision_id'))
            AND l.income_account_id=NEW.income_account_id)))
 AND (NEW.item_id IS NULL OR EXISTS(SELECT 1 FROM directory_item t WHERE t.id=NEW.item_id AND t.active=1)
      OR (r.state<>'OPEN' AND EXISTS(SELECT 1 FROM sales_invoicerevision p JOIN sales_invoiceline l ON l.revision_id=p.id
          WHERE p.invoice_id=r.invoice_id AND p.number=r.number-1
            AND l.position=CASE WHEN r.state='DRAFT' THEN json_extract(NEW.data,'$.origin_position') ELSE NEW.position END
            AND (r.state<>'DRAFT' OR p.id=json_extract(NEW.data,'$.origin_revision_id'))
            AND l.item_id=NEW.item_id))));
END;

-- SQL validates object boundaries and relational identities. The registered,
-- pure Rust functions validate every mandatory nested field and snapshot id.
CREATE VIEW sales_snapshot_violations AS
 SELECT 'invoice snapshot: '||r.id problem FROM sales_invoicerevision r
 JOIN sales_invoice i ON i.id=r.invoice_id
 WHERE NOT json_valid(r.data) OR json_type(r.data) IS NOT 'object'
   OR json_type(r.data,'$.customer_snapshot') IS NOT 'object'
   OR json_type(r.data,'$.company_snapshot') IS NOT 'object'
   OR json_extract(r.data,'$.company_snapshot.id') IS NOT i.company_id
   OR NOT fb_sales_header_ok('invoice',r.data)
 UNION ALL
 SELECT 'invoice line snapshot: '||l.id FROM sales_invoiceline l
 WHERE NOT json_valid(l.data) OR json_type(l.data) IS NOT 'object'
   OR json_type(l.data,'$.expense_account_id') IS NULL
   OR json_type(l.data,'$.account_snapshot') IS NOT 'object'
   OR json_type(l.data,'$.item_snapshot') IS NOT 'object' OR NOT fb_sales_line_ok(l.data)
 UNION ALL
 SELECT 'receipt snapshot: '||r.id FROM sales_receipt r
 WHERE NOT json_valid(r.data) OR json_type(r.data) IS NOT 'object'
   OR json_type(r.data,'$.customer_snapshot') IS NOT 'object'
   OR json_type(r.data,'$.company_snapshot') IS NOT 'object'
   OR json_extract(r.data,'$.company_snapshot.id') IS NOT r.company_id
   OR NOT fb_sales_header_ok('receipt',r.data)
 UNION ALL
 SELECT 'sale snapshot: '||s.id FROM sales_salesreceipt s
 WHERE NOT json_valid(s.data) OR json_type(s.data) IS NOT 'object'
   OR json_type(s.data,'$.tax_snapshot') IS NOT 'object' OR NOT fb_sales_snapshot_ok('detail',s.data)
 UNION ALL
 SELECT 'sale line snapshot: '||l.id FROM sales_salesreceiptline l
 WHERE NOT json_valid(l.data) OR json_type(l.data) IS NOT 'object'
   OR json_type(l.data,'$.account_snapshot') IS NOT 'object'
   OR json_type(l.data,'$.item_snapshot') IS NOT 'object' OR NOT fb_sales_line_ok(l.data)
 UNION ALL
 SELECT 'receipt reversal snapshot: '||z.id FROM sales_receiptreversal z
 WHERE NOT json_valid(z.data) OR json_type(z.data) IS NOT 'object' OR NOT fb_sales_reversal_ok(z.data)
   OR json_extract(z.data,'$.id') IS NOT z.id;

CREATE TRIGGER fb_sales_revision_snapshot BEFORE INSERT ON sales_invoicerevision
WHEN NOT EXISTS(SELECT 1 FROM sales_invoicerevision WHERE id=NEW.id OR (invoice_id=NEW.invoice_id AND number=NEW.number)) AND EXISTS(SELECT 1 FROM sales_invoice WHERE id=NEW.invoice_id)
BEGIN
 SELECT RAISE(ABORT,'invoice_revision_fields: Invalid invoice snapshots') WHERE NOT json_valid(NEW.data)
   OR json_type(NEW.data) IS NOT 'object' OR NOT fb_sales_header_ok('invoice',NEW.data)
   OR json_extract(NEW.data,'$.company_snapshot.id') IS NOT
      (SELECT company_id FROM sales_invoice WHERE id=NEW.invoice_id);
END;
CREATE TRIGGER fb_sales_line_snapshot BEFORE INSERT ON sales_invoiceline
WHEN NOT EXISTS(SELECT 1 FROM sales_invoiceline WHERE id=NEW.id OR (revision_id=NEW.revision_id AND position=NEW.position)) AND EXISTS(SELECT 1 FROM sales_invoicerevision WHERE id=NEW.revision_id)
BEGIN
 SELECT RAISE(ABORT,'sales_snapshot: Invalid invoice line snapshots') WHERE NOT json_valid(NEW.data)
   OR json_type(NEW.data,'$.expense_account_id') IS NULL
   OR json_type(NEW.data,'$.account_snapshot') IS NOT 'object'
   OR json_type(NEW.data,'$.item_snapshot') IS NOT 'object' OR NOT fb_sales_snapshot_ok('line',NEW.data);
END;
CREATE TRIGGER fb_sales_receipt_snapshot BEFORE INSERT ON sales_receipt
WHEN NOT EXISTS(SELECT 1 FROM sales_receipt WHERE id=NEW.id OR identity_id=NEW.identity_id OR journal_id=NEW.journal_id) AND EXISTS(SELECT 1 FROM directory_customer WHERE id=NEW.customer_id AND company_id=NEW.company_id)
BEGIN
 SELECT RAISE(ABORT,'receipt_fields: Invalid receipt snapshots') WHERE NOT json_valid(NEW.data)
   OR json_type(NEW.data) IS NOT 'object' OR NOT fb_sales_header_ok('receipt',NEW.data)
   OR json_extract(NEW.data,'$.company_snapshot.id') IS NOT NEW.company_id;
END;
CREATE TRIGGER fb_sales_detail_snapshot BEFORE INSERT ON sales_salesreceipt
WHEN NOT EXISTS(SELECT 1 FROM sales_salesreceipt WHERE id=NEW.id OR receipt_id=NEW.receipt_id) AND EXISTS(SELECT 1 FROM sales_receipt WHERE id=NEW.receipt_id AND kind='SR')
BEGIN
 SELECT RAISE(ABORT,'sales_snapshot: Invalid sale snapshots') WHERE NOT json_valid(NEW.data)
   OR json_type(NEW.data) IS NOT 'object' OR json_type(NEW.data,'$.tax_snapshot') IS NOT 'object' OR NOT fb_sales_snapshot_ok('detail',NEW.data);
END;
CREATE TRIGGER fb_sales_sale_line_snapshot BEFORE INSERT ON sales_salesreceiptline
WHEN NOT EXISTS(SELECT 1 FROM sales_salesreceiptline WHERE id=NEW.id OR (sale_id=NEW.sale_id AND position=NEW.position)) AND EXISTS(SELECT 1 FROM sales_salesreceipt WHERE id=NEW.sale_id)
BEGIN
 SELECT RAISE(ABORT,'sales_snapshot: Invalid sale line snapshots') WHERE NOT json_valid(NEW.data)
   OR json_type(NEW.data,'$.account_snapshot') IS NOT 'object'
   OR json_type(NEW.data,'$.item_snapshot') IS NOT 'object' OR NOT fb_sales_snapshot_ok('line',NEW.data);
END;
CREATE TRIGGER fb_sales_reversal_snapshot BEFORE INSERT ON sales_receiptreversal
WHEN NOT EXISTS(SELECT 1 FROM sales_receiptreversal WHERE id=NEW.id OR receipt_id=NEW.receipt_id OR journal_id=NEW.journal_id) AND EXISTS(SELECT 1 FROM sales_receipt WHERE id=NEW.receipt_id)
BEGIN
 SELECT RAISE(ABORT,'receipt_reversal_link: Invalid reversal snapshots') WHERE NOT json_valid(NEW.data)
   OR json_type(NEW.data) IS NOT 'object' OR NOT fb_sales_reversal_ok(NEW.data)
   OR json_extract(NEW.data,'$.id') IS NOT NEW.id;
END;
CREATE TRIGGER fb_sales_snapshot_seal BEFORE INSERT ON sales_seal
WHEN NEW.kind IN('sales_invoice_revision','sales_receipt','sales_receipt_reversal')
BEGIN
 SELECT RAISE(ABORT,'sales_snapshot: Invalid stored sales snapshots')
   WHERE EXISTS(SELECT 1 FROM sales_snapshot_violations);
 SELECT RAISE(ABORT,'invoice_line_origin: Invalid retained line provenance')
   WHERE EXISTS(SELECT 1 FROM sales_origin_violations v
     JOIN sales_invoiceline l ON l.id=v.id JOIN sales_invoicerevision r ON r.id=l.revision_id
     WHERE r.state='DRAFT');
 -- OPEN/VOID copy failures keep the existing invoice_financial_immutable
 -- insertion/seal diagnostic; the view also checks those copies on open.
END;
