-- The 0070 receipt rebuild dropped this reviewed 0016 INSERT boundary.
CREATE TRIGGER fb_sales_receipt_snapshot BEFORE INSERT ON sales_receipt
WHEN NOT EXISTS(SELECT 1 FROM sales_receipt WHERE id=NEW.id OR identity_id=NEW.identity_id OR journal_id=NEW.journal_id) AND EXISTS(SELECT 1 FROM directory_customer WHERE id=NEW.customer_id AND company_id=NEW.company_id)
BEGIN
 SELECT RAISE(ABORT,'receipt_fields: Invalid receipt snapshots') WHERE NOT json_valid(NEW.data)
   OR json_type(NEW.data) IS NOT 'object' OR NOT fb_sales_header_ok('receipt',NEW.data)
   OR json_extract(NEW.data,'$.company_snapshot.id') IS NOT NEW.company_id;
END;
