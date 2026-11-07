-- Offline AI preferences and authoritative receipt readings. Credentials remain in the plugin vault.
CREATE TABLE assistant_connection (
 id TEXT PRIMARY KEY CHECK(fb_banking_uuid(id)),
 company_id TEXT NOT NULL UNIQUE REFERENCES ledger_company(id) DEFERRABLE INITIALLY DEFERRED,
 created_by_id TEXT NOT NULL REFERENCES fb_user(id) DEFERRABLE INITIALLY DEFERRED,
 mode TEXT NOT NULL DEFAULT 'siwc' CHECK(mode IN ('siwc','api_key')),
 status TEXT NOT NULL DEFAULT 'disconnected' CHECK(status IN ('connected','needs_signin','disconnected')),
 credential_present INTEGER NOT NULL DEFAULT 0 CHECK(credential_present IN (0,1)),
 account_email TEXT NOT NULL DEFAULT '' CHECK(length(account_email)<=254),
 key_last4 TEXT NOT NULL DEFAULT '' CHECK(length(key_last4)<=4),
 model TEXT NOT NULL DEFAULT '' CHECK(length(model)<=100),
 model_choices TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(model_choices)) CHECK(fb_assistant_json('choices',model_choices)),
 plan_usage INTEGER NOT NULL DEFAULT 0 CHECK(plan_usage IN (0,1)),
 notice_pending INTEGER NOT NULL DEFAULT 0 CHECK(notice_pending IN (0,1)),
 notice_shown INTEGER NOT NULL DEFAULT 0 CHECK(notice_shown IN (0,1)),
 read_attachments INTEGER NOT NULL DEFAULT 0 CHECK(read_attachments IN (0,1)),
 revoke_confirmed INTEGER CHECK(revoke_confirmed IN (0,1)),
 last_error TEXT NOT NULL DEFAULT '' CHECK(length(last_error)<=300),
 token_expires_at INTEGER CHECK(token_expires_at>=0),
 last_used_at INTEGER CHECK(last_used_at>=0),
 generation INTEGER NOT NULL DEFAULT 0 CHECK(generation>=0),
 CHECK(status<>'disconnected' OR credential_present=0)
) STRICT, WITHOUT ROWID;
CREATE TABLE assistant_reading (
 id TEXT PRIMARY KEY CHECK(fb_banking_uuid(id)),
 company_id TEXT NOT NULL REFERENCES ledger_company(id) DEFERRABLE INITIALLY DEFERRED,
 created_by_id TEXT NOT NULL REFERENCES fb_user(id) DEFERRABLE INITIALLY DEFERRED,
 request_key TEXT NOT NULL CHECK(fb_banking_uuid(request_key)),
 source_file_id TEXT NOT NULL CHECK(fb_banking_uuid(source_file_id)),
 bill_id TEXT REFERENCES payables_bill(id) DEFERRABLE INITIALLY DEFERRED,
 origin_bill_id TEXT REFERENCES payables_bill(id) DEFERRABLE INITIALLY DEFERRED,
 original_name TEXT NOT NULL CHECK(length(original_name)<=120),
 content_type TEXT NOT NULL CHECK(content_type IN ('application/pdf','image/png','image/jpeg','image/webp')),
 size INTEGER NOT NULL CHECK(size>0 AND size<=10485760),
 sha256 TEXT NOT NULL CHECK(length(sha256)=64 AND sha256 NOT GLOB '*[^0-9a-f]*'),
 status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','ok','failed')),
 mode TEXT NOT NULL CHECK(mode IN ('','siwc','api_key')),
 model TEXT NOT NULL CHECK(length(model)<=100),
 result TEXT NOT NULL DEFAULT '{}' CHECK(json_valid(result)),
 error TEXT NOT NULL DEFAULT '' CHECK(length(error)<=300),
 created_at INTEGER NOT NULL CHECK(created_at>=0),
 finished_at INTEGER CHECK(finished_at>=created_at),
 attempt INTEGER NOT NULL DEFAULT 1 CHECK(attempt>0),
 connection_generation INTEGER NOT NULL CHECK(connection_generation>=0),
 UNIQUE(company_id,request_key),
 CHECK(origin_bill_id IS NULL OR (bill_id IS NOT NULL AND origin_bill_id=bill_id)),
 CHECK((status='ok' AND fb_assistant_json('result',result) AND finished_at IS NOT NULL AND error='')
    OR (status<>'ok' AND result='{}')),
 CHECK(status<>'pending' OR (finished_at IS NULL AND error=''))
) STRICT, WITHOUT ROWID;
CREATE INDEX assistant_reading_rate ON assistant_reading(company_id,created_by_id,created_at);
CREATE VIEW assistant_invalid AS
 SELECT id FROM assistant_connection c WHERE
  NOT EXISTS(SELECT 1 FROM ledger_company p WHERE p.id=c.company_id)
  OR NOT EXISTS(SELECT 1 FROM fb_user u WHERE u.id=c.created_by_id)
  OR NOT fb_assistant_json('choices',c.model_choices)
 UNION ALL SELECT id FROM assistant_reading r WHERE
  NOT EXISTS(SELECT 1 FROM ledger_company p WHERE p.id=r.company_id)
  OR NOT EXISTS(SELECT 1 FROM fb_user u WHERE u.id=r.created_by_id)
  OR NOT EXISTS(SELECT 1 FROM assistant_connection c WHERE c.company_id=r.company_id)
  OR (r.bill_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM payables_bill b WHERE b.id=r.bill_id AND b.company_id=r.company_id))
  OR (r.status='ok' AND (NOT fb_assistant_json('result',r.result)
       OR json_extract(r.result,'$.currency') IS NOT (SELECT currency FROM ledger_company p WHERE p.id=r.company_id)));
CREATE TRIGGER fb_assistant_connection_insert BEFORE INSERT ON assistant_connection BEGIN
 SELECT RAISE(ABORT,'assistant_connection: Protected key already exists') WHERE EXISTS(SELECT 1 FROM assistant_connection WHERE id=NEW.id OR company_id=NEW.company_id);
 SELECT RAISE(ABORT,'assistant_connection: Existing company and operator required') WHERE
  NOT EXISTS(SELECT 1 FROM ledger_company WHERE id=NEW.company_id) OR NOT EXISTS(SELECT 1 FROM fb_user WHERE id=NEW.created_by_id);
END;
CREATE TRIGGER fb_assistant_connection_update BEFORE UPDATE ON assistant_connection BEGIN
 SELECT RAISE(ABORT,'assistant_connection: Identity and ownership are permanent') WHERE NEW.id IS NOT OLD.id OR NEW.company_id IS NOT OLD.company_id OR NEW.created_by_id IS NOT OLD.created_by_id;
END;
CREATE TRIGGER fb_assistant_connection_delete BEFORE DELETE ON assistant_connection BEGIN
 SELECT RAISE(ABORT,'assistant_connection: Disconnect instead of deleting');
END;
CREATE TRIGGER fb_assistant_reading_insert BEFORE INSERT ON assistant_reading BEGIN
 SELECT RAISE(ABORT,'assistant_reading: Protected key already exists') WHERE EXISTS(SELECT 1 FROM assistant_reading WHERE id=NEW.id OR (company_id=NEW.company_id AND request_key=NEW.request_key));
 SELECT RAISE(ABORT,'assistant_reading: Existing eligible parents required') WHERE
  NOT EXISTS(SELECT 1 FROM fb_user WHERE id=NEW.created_by_id) OR
  NOT EXISTS(SELECT 1 FROM assistant_connection WHERE company_id=NEW.company_id) OR
  (NEW.bill_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM payables_bill b JOIN payables_billrevision r ON r.bill_id=b.id AND r.number=b.revision WHERE b.id=NEW.bill_id AND b.company_id=NEW.company_id AND r.state='DRAFT'));
 SELECT RAISE(ABORT,'assistant_reading: Company currency mismatch') WHERE NEW.status='ok' AND json_extract(NEW.result,'$.currency') IS NOT (SELECT currency FROM ledger_company WHERE id=NEW.company_id);
END;
CREATE TRIGGER fb_assistant_reading_update BEFORE UPDATE ON assistant_reading BEGIN
 SELECT RAISE(ABORT,'assistant_reading: Identity and provenance are permanent') WHERE
  NEW.id IS NOT OLD.id OR NEW.company_id IS NOT OLD.company_id OR NEW.created_by_id IS NOT OLD.created_by_id OR NEW.request_key IS NOT OLD.request_key OR NEW.origin_bill_id IS NOT OLD.origin_bill_id OR NEW.sha256 IS NOT OLD.sha256;
 SELECT RAISE(ABORT,'assistant_reading: File provenance changes only on expired upload retry') WHERE
  (NEW.source_file_id IS NOT OLD.source_file_id OR NEW.original_name IS NOT OLD.original_name OR NEW.content_type IS NOT OLD.content_type OR NEW.size IS NOT OLD.size)
  AND NOT (OLD.status='failed' AND OLD.finished_at IS NULL AND OLD.bill_id IS NULL AND NEW.status='pending' AND NEW.attempt=OLD.attempt+1);
 SELECT RAISE(ABORT,'assistant_reading: Completed evidence is immutable') WHERE
  OLD.finished_at IS NOT NULL AND (NEW.result IS NOT OLD.result OR NEW.status IS NOT OLD.status OR NEW.error IS NOT OLD.error OR NEW.finished_at IS NOT OLD.finished_at OR NEW.created_at IS NOT OLD.created_at OR NEW.attempt IS NOT OLD.attempt OR NEW.mode IS NOT OLD.mode OR NEW.model IS NOT OLD.model OR NEW.connection_generation IS NOT OLD.connection_generation);
 SELECT RAISE(ABORT,'assistant_reading: Bill ownership is permanent') WHERE OLD.bill_id IS NOT NULL AND NEW.bill_id IS NOT OLD.bill_id;
 SELECT RAISE(ABORT,'assistant_reading: Existing draft bill required') WHERE NEW.bill_id IS NOT OLD.bill_id AND NOT EXISTS(SELECT 1 FROM payables_bill b JOIN payables_billrevision r ON r.bill_id=b.id AND r.number=b.revision WHERE b.id=NEW.bill_id AND b.company_id=NEW.company_id AND r.state='DRAFT');
 SELECT RAISE(ABORT,'assistant_reading: Company currency mismatch') WHERE NEW.status='ok' AND json_extract(NEW.result,'$.currency') IS NOT (SELECT currency FROM ledger_company WHERE id=NEW.company_id);
END;
CREATE TRIGGER fb_assistant_reading_delete BEFORE DELETE ON assistant_reading BEGIN
 SELECT RAISE(ABORT,'assistant_reading: Reading history cannot be deleted');
END;
CREATE TRIGGER fb_assistant_user_delete BEFORE DELETE ON fb_user WHEN EXISTS(SELECT 1 FROM assistant_connection WHERE created_by_id=OLD.id) OR EXISTS(SELECT 1 FROM assistant_reading WHERE created_by_id=OLD.id) BEGIN
 SELECT RAISE(ABORT,'assistant_parent: Referenced operator cannot be deleted');
END;
CREATE TRIGGER fb_assistant_company_delete BEFORE DELETE ON ledger_company WHEN EXISTS(SELECT 1 FROM assistant_connection WHERE company_id=OLD.id) BEGIN
 SELECT RAISE(ABORT,'assistant_parent: Referenced company cannot be deleted');
END;
CREATE TRIGGER fb_assistant_bill_delete BEFORE DELETE ON payables_bill WHEN EXISTS(SELECT 1 FROM assistant_reading WHERE bill_id=OLD.id) BEGIN
 SELECT RAISE(ABORT,'assistant_parent: Referenced bill cannot be deleted');
END;
