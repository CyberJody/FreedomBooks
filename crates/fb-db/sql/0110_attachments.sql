-- Attachment rows retain encrypted blobs, including soft removals (backup/audit references).
CREATE TABLE attachments_attachment (
 id TEXT PRIMARY KEY CHECK(length(id)=36 AND substr(id,9,1)='-' AND substr(id,14,1)='-' AND substr(id,19,1)='-' AND substr(id,24,1)='-' AND length(replace(id,'-',''))=32 AND replace(id,'-','') NOT GLOB '*[^0-9a-f]*'), company_id TEXT NOT NULL REFERENCES ledger_company(id),
 document_id TEXT REFERENCES documents_documentidentity(id), journal_id TEXT REFERENCES ledger_journal(id),
 original_name TEXT NOT NULL CHECK(length(trim(original_name)) BETWEEN 1 AND 120),
 stored_name TEXT NOT NULL CHECK(length(stored_name)=36 AND substr(stored_name,33)='.bin' AND substr(stored_name,1,32) NOT GLOB '*[^0-9a-f]*'),
 content_type TEXT NOT NULL CHECK(content_type IN('application/pdf','image/png','image/jpeg','image/webp')),
 size INTEGER NOT NULL CHECK(size BETWEEN 1 AND 104857600),
 sha256 TEXT NOT NULL CHECK(length(sha256)=64 AND sha256 NOT GLOB '*[^0-9a-f]*'),
 original_sha256 TEXT NOT NULL CHECK(length(original_sha256)=64 AND original_sha256 NOT GLOB '*[^0-9a-f]*'),
 wrapped_key TEXT NOT NULL CHECK(length(wrapped_key)=96 AND wrapped_key NOT GLOB '*[^0-9a-f]*'),
 wrap_nonce TEXT NOT NULL CHECK(length(wrap_nonce)=24 AND wrap_nonce NOT GLOB '*[^0-9a-f]*'),
 nonce TEXT NOT NULL CHECK(length(nonce)=14 AND nonce NOT GLOB '*[^0-9a-f]*'),
 uploaded_by_id TEXT NOT NULL REFERENCES fb_user(id), uploaded_by_name TEXT NOT NULL CHECK(length(trim(uploaded_by_name)) BETWEEN 1 AND 150),
 uploaded_at TEXT NOT NULL CHECK(julianday(uploaded_at) IS NOT NULL),
 uploaded_number TEXT NOT NULL CHECK(length(uploaded_number)>0), deleted_number TEXT NOT NULL DEFAULT '',
 request_key TEXT NOT NULL CHECK(length(request_key)=36 AND substr(request_key,9,1)='-' AND substr(request_key,14,1)='-' AND substr(request_key,19,1)='-' AND substr(request_key,24,1)='-' AND length(replace(request_key,'-',''))=32 AND replace(request_key,'-','') NOT GLOB '*[^0-9a-f]*'),
 deleted INTEGER NOT NULL DEFAULT 0 CHECK(deleted IN(0,1)), deleted_by_id TEXT REFERENCES fb_user(id),
 deleted_by_name TEXT NOT NULL DEFAULT '', deleted_at TEXT, delete_reason TEXT NOT NULL DEFAULT '',
 UNIQUE(company_id,request_key),
 CHECK((document_id IS NOT NULL AND journal_id IS NULL) OR (journal_id IS NOT NULL AND document_id IS NULL)),
 CHECK((deleted=0 AND deleted_by_id IS NULL AND deleted_at IS NULL AND deleted_by_name='' AND delete_reason='' AND deleted_number='') OR
 (deleted=1 AND deleted_by_id IS NOT NULL AND julianday(deleted_at) IS NOT NULL AND length(trim(deleted_by_name)) BETWEEN 1 AND 150 AND length(trim(delete_reason)) BETWEEN 1 AND 500 AND length(deleted_number)>0))
) STRICT, WITHOUT ROWID;
CREATE INDEX attachment_document_idx ON attachments_attachment(company_id,document_id,deleted);
CREATE INDEX attachment_journal_idx ON attachments_attachment(company_id,journal_id,deleted);
CREATE INDEX attachment_uploader_idx ON attachments_attachment(company_id,uploaded_by_id,uploaded_at);
CREATE INDEX attachment_fingerprint_idx ON attachments_attachment(company_id,sha256);
CREATE INDEX attachment_stored_idx ON attachments_attachment(stored_name);
CREATE VIEW attachment_file_refs AS SELECT company_id,stored_name,count(*) AS reference_count FROM attachments_attachment GROUP BY company_id,stored_name;
CREATE VIEW attachment_invalid AS
 SELECT a.id FROM attachments_attachment a WHERE
 NOT EXISTS(SELECT 1 FROM ledger_company c WHERE c.id=a.company_id) OR
 NOT EXISTS(SELECT 1 FROM fb_user u WHERE u.id=a.uploaded_by_id) OR
 (a.deleted_by_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM fb_user u WHERE u.id=a.deleted_by_id)) OR
 NOT ((a.document_id IS NOT NULL AND a.journal_id IS NULL AND EXISTS(SELECT 1 FROM documents_documentidentity d WHERE d.id=a.document_id AND d.company_id=a.company_id AND d.kind IN('EST','INV','RCP','SR','BILL','BPAY','DEP','CHK','TRF','CM','RFD','CDP','CDR'))) OR
 (a.journal_id IS NOT NULL AND a.document_id IS NULL AND EXISTS(SELECT 1 FROM ledger_journal j WHERE j.id=a.journal_id AND j.company_id=a.company_id AND j.source_id IS NULL AND j.reversal_of_id IS NULL))) OR
 EXISTS(SELECT 1 FROM attachments_attachment b WHERE b.id<>a.id AND ((b.stored_name=a.stored_name AND (b.company_id<>a.company_id OR b.sha256<>a.sha256)) OR (b.company_id=a.company_id AND b.sha256=a.sha256 AND (b.stored_name<>a.stored_name OR b.content_type<>a.content_type OR b.size<>a.size OR b.wrapped_key<>a.wrapped_key OR b.wrap_nonce<>a.wrap_nonce OR b.nonce<>a.nonce))))
 UNION ALL SELECT min(id) FROM attachments_attachment WHERE deleted=0 GROUP BY company_id,document_id,journal_id HAVING count(*)>20;
CREATE TRIGGER attachment_guard_insert BEFORE INSERT ON attachments_attachment BEGIN
 SELECT RAISE(ABORT,'attachment_keys: Protected key already exists') WHERE EXISTS(SELECT 1 FROM attachments_attachment WHERE id=NEW.id OR (company_id=NEW.company_id AND request_key=NEW.request_key));
 SELECT RAISE(ABORT,'attachment_owner: An attachment must belong to a document or manual journal entry of the same company') WHERE
 NOT ((NEW.document_id IS NOT NULL AND NEW.journal_id IS NULL AND EXISTS(SELECT 1 FROM documents_documentidentity WHERE id=NEW.document_id AND company_id=NEW.company_id AND kind IN('EST','INV','RCP','SR','BILL','BPAY','DEP','CHK','TRF','CM','RFD','CDP','CDR'))) OR
 (NEW.journal_id IS NOT NULL AND NEW.document_id IS NULL AND EXISTS(SELECT 1 FROM ledger_journal WHERE id=NEW.journal_id AND company_id=NEW.company_id AND source_id IS NULL AND reversal_of_id IS NULL)));
 SELECT RAISE(ABORT,'attachment_actor: Attachment operators must exist') WHERE NOT EXISTS(SELECT 1 FROM fb_user WHERE id=NEW.uploaded_by_id);
 SELECT RAISE(ABORT,'attachment_insert_active: A new attachment cannot start out removed') WHERE NEW.deleted<>0 OR NEW.deleted_by_id IS NOT NULL OR NEW.deleted_at IS NOT NULL OR NEW.delete_reason<>'' OR NEW.deleted_by_name<>'' OR NEW.deleted_number<>'';
 SELECT RAISE(ABORT,'attachment_limit: At most 20 attachments per document') WHERE (SELECT count(*) FROM attachments_attachment WHERE company_id=NEW.company_id AND deleted=0 AND document_id IS NEW.document_id AND journal_id IS NEW.journal_id)>=20;
 SELECT RAISE(ABORT,'attachment_blob: Inconsistent encrypted file reference') WHERE EXISTS(SELECT 1 FROM attachments_attachment b WHERE
 (b.stored_name=NEW.stored_name AND (b.company_id<>NEW.company_id OR b.sha256<>NEW.sha256)) OR
 (b.company_id=NEW.company_id AND b.sha256=NEW.sha256 AND (b.stored_name<>NEW.stored_name OR b.content_type<>NEW.content_type OR b.size<>NEW.size OR b.wrapped_key<>NEW.wrapped_key OR b.wrap_nonce<>NEW.wrap_nonce OR b.nonce<>NEW.nonce)));
END;
CREATE TRIGGER attachment_primary_key_permanent BEFORE UPDATE OF id ON attachments_attachment WHEN NEW.id IS NOT OLD.id BEGIN SELECT RAISE(ABORT,'primary_key_immutable: Primary keys never change'); END;
CREATE TRIGGER attachment_guard_update BEFORE UPDATE ON attachments_attachment BEGIN
 SELECT RAISE(ABORT,'attachment_keys: Update would replace an existing row') WHERE EXISTS(SELECT 1 FROM attachments_attachment WHERE id<>OLD.id AND (id=NEW.id OR (company_id=NEW.company_id AND request_key=NEW.request_key)));
 SELECT RAISE(ABORT,'attachment_immutable: Attachment history is immutable: the only change allowed is removing it with a reason') WHERE OLD.deleted=1 OR NEW.deleted<>1 OR NEW.deleted_by_id IS NULL OR NOT EXISTS(SELECT 1 FROM fb_user WHERE id=NEW.deleted_by_id) OR NEW.deleted_at IS NULL OR julianday(NEW.deleted_at) IS NULL OR length(trim(NEW.delete_reason)) NOT BETWEEN 1 AND 500 OR length(trim(NEW.deleted_by_name)) NOT BETWEEN 1 AND 150 OR NEW.id IS NOT OLD.id OR NEW.company_id IS NOT OLD.company_id OR NEW.document_id IS NOT OLD.document_id OR NEW.journal_id IS NOT OLD.journal_id OR NEW.original_name IS NOT OLD.original_name OR NEW.stored_name IS NOT OLD.stored_name OR NEW.content_type IS NOT OLD.content_type OR NEW.size IS NOT OLD.size OR NEW.sha256 IS NOT OLD.sha256 OR NEW.original_sha256 IS NOT OLD.original_sha256 OR NEW.wrapped_key IS NOT OLD.wrapped_key OR NEW.wrap_nonce IS NOT OLD.wrap_nonce OR NEW.nonce IS NOT OLD.nonce OR NEW.uploaded_by_id IS NOT OLD.uploaded_by_id OR NEW.uploaded_by_name IS NOT OLD.uploaded_by_name OR NEW.uploaded_at IS NOT OLD.uploaded_at OR NEW.request_key IS NOT OLD.request_key OR NEW.uploaded_number IS NOT OLD.uploaded_number;
END;
CREATE TRIGGER attachment_guard_delete BEFORE DELETE ON attachments_attachment BEGIN SELECT RAISE(ABORT,'attachment_immutable: Attachments are never deleted; remove them with a reason instead'); END;
CREATE TRIGGER attachment_added_due AFTER INSERT ON attachments_attachment BEGIN INSERT INTO fb_seal_due VALUES('attachment_added',NEW.id); END;
CREATE TRIGGER attachment_removed_due AFTER UPDATE ON attachments_attachment BEGIN INSERT INTO fb_seal_due VALUES('attachment_removed',NEW.id); END;
CREATE VIEW attachment_owner_details AS
 SELECT a.id,
 CASE WHEN a.journal_id IS NOT NULL THEN 'journal:'||a.journal_id ELSE CASE d.kind
 WHEN 'EST' THEN 'estimate:'||d.id
 WHEN 'INV' THEN 'invoice:'||(SELECT id FROM sales_invoice WHERE identity_id=d.id)
 WHEN 'RCP' THEN 'receipt:'||(SELECT id FROM sales_receipt WHERE identity_id=d.id)
 WHEN 'SR' THEN 'receipt:'||(SELECT id FROM sales_receipt WHERE identity_id=d.id)
 WHEN 'CDP' THEN 'receipt:'||(SELECT id FROM sales_receipt WHERE identity_id=d.id)
 WHEN 'BILL' THEN 'bill:'||(SELECT id FROM payables_bill WHERE identity_id=d.id)
 WHEN 'BPAY' THEN 'billpayment:'||(SELECT id FROM payables_billpayment WHERE identity_id=d.id)
 WHEN 'DEP' THEN 'deposit:'||(SELECT id FROM banking_deposit WHERE identity_id=d.id)
 WHEN 'CHK' THEN 'check:'||(SELECT id FROM checks_check WHERE identity_id=d.id)
 WHEN 'TRF' THEN 'transfer:'||(SELECT id FROM transfers_transfer WHERE identity_id=d.id)
 WHEN 'CM' THEN 'creditmemo:'||(SELECT id FROM credits_creditmemo WHERE identity_id=d.id)
 WHEN 'RFD' THEN 'creditrefund:'||(SELECT id FROM credits_creditrefund WHERE identity_id=d.id)
 WHEN 'CDR' THEN 'depositrefund:'||(SELECT id FROM prepayments_depositrefund WHERE identity_id=d.id)
 END END AS target,
 CASE WHEN a.journal_id IS NOT NULL THEN CASE WHEN j.entry_number='' THEN printf('J%06d',j.number) ELSE j.entry_number END ELSE d.number END AS number
 FROM attachments_attachment a LEFT JOIN documents_documentidentity d ON d.id=a.document_id LEFT JOIN ledger_journal j ON j.id=a.journal_id;
CREATE TRIGGER attachment_snapshot_insert AFTER INSERT ON attachments_attachment BEGIN
 SELECT RAISE(ABORT,'attachment_number: Attachment number snapshot must match its owner') WHERE NOT EXISTS(SELECT 1 FROM attachment_owner_details WHERE id=NEW.id AND number=NEW.uploaded_number);
END;
CREATE TRIGGER attachment_snapshot_remove BEFORE UPDATE ON attachments_attachment BEGIN
 SELECT RAISE(ABORT,'attachment_number: Attachment number snapshot must match its owner') WHERE NOT EXISTS(SELECT 1 FROM attachment_owner_details WHERE id=OLD.id AND number=NEW.deleted_number);
END;
CREATE VIEW attachment_audit_evidence AS
 SELECT a.id, kinds.kind FROM attachments_attachment a JOIN attachment_owner_details o ON o.id=a.id
 CROSS JOIN (SELECT 'attachment_added' AS kind UNION ALL SELECT 'attachment_removed') kinds
 WHERE (kinds.kind='attachment_added' OR a.deleted=1) AND
 (SELECT count(*) FROM ledger_auditevent e WHERE e.action=CASE kinds.kind WHEN 'attachment_added' THEN 'attachment.added' ELSE 'attachment.removed' END AND json_extract(e.detail,'$.attachment')=a.id)=1 AND
 EXISTS(SELECT 1 FROM ledger_auditevent e WHERE e.company_id=a.company_id
 AND e.action=CASE kinds.kind WHEN 'attachment_added' THEN 'attachment.added' ELSE 'attachment.removed' END
 AND e.actor_id=CASE kinds.kind WHEN 'attachment_added' THEN a.uploaded_by_id ELSE a.deleted_by_id END
 AND e.actor_name=CASE kinds.kind WHEN 'attachment_added' THEN a.uploaded_by_name ELSE a.deleted_by_name END
 AND e.target=o.target AND json_type(e.detail,'$.number')='text' AND json_extract(e.detail,'$.number')=CASE kinds.kind WHEN 'attachment_added' THEN a.uploaded_number ELSE a.deleted_number END
 AND json_type(e.detail,'$.attachment')='text' AND json_extract(e.detail,'$.attachment')=a.id
 AND json_type(e.detail,'$.name')='text' AND json_extract(e.detail,'$.name')=a.original_name
 AND json_type(e.detail,'$.content_type')='text' AND json_extract(e.detail,'$.content_type')=a.content_type
 AND json_type(e.detail,'$.size')='integer' AND json_extract(e.detail,'$.size')=a.size
 AND json_type(e.detail,'$.sha256')='text' AND json_extract(e.detail,'$.sha256')=a.sha256
 AND CASE WHEN kinds.kind='attachment_removed' THEN json_type(e.detail,'$.reason')='text' AND json_extract(e.detail,'$.reason')=a.delete_reason AND json_type(e.detail,'$.original_sha256') IS NULL
 WHEN a.original_sha256<>a.sha256 THEN json_type(e.detail,'$.original_sha256')='text' AND json_extract(e.detail,'$.original_sha256')=a.original_sha256
 ELSE json_type(e.detail,'$.original_sha256') IS NULL END);
CREATE VIEW attachment_audit_invalid AS
 SELECT id FROM attachments_attachment a WHERE NOT EXISTS(SELECT 1 FROM attachment_audit_evidence e WHERE e.id=a.id AND e.kind='attachment_added')
 OR (a.deleted=1 AND NOT EXISTS(SELECT 1 FROM attachment_audit_evidence e WHERE e.id=a.id AND e.kind='attachment_removed'))
 OR (a.deleted=0 AND EXISTS(SELECT 1 FROM ledger_auditevent e WHERE e.action='attachment.removed' AND json_extract(e.detail,'$.attachment')=a.id));
CREATE TRIGGER attachment_audit_seal BEFORE INSERT ON fb_seal WHEN NEW.kind IN('attachment_added','attachment_removed') BEGIN
 SELECT RAISE(ABORT,'attachment_audit: Attachment audit evidence is required') WHERE NOT EXISTS(SELECT 1 FROM attachment_audit_evidence WHERE id=NEW.doc_id AND kind=NEW.kind);
END;
CREATE TRIGGER attachment_document_id_kept BEFORE DELETE ON documents_documentidentity WHEN EXISTS(SELECT 1 FROM attachments_attachment WHERE document_id=OLD.id) BEGIN SELECT RAISE(ABORT,'attachment_parent: Attachment parents cannot be deleted'); END;
CREATE TRIGGER attachment_journal_id_kept BEFORE DELETE ON ledger_journal WHEN EXISTS(SELECT 1 FROM attachments_attachment WHERE journal_id=OLD.id) BEGIN SELECT RAISE(ABORT,'attachment_parent: Attachment parents cannot be deleted'); END;
CREATE TRIGGER attachment_company_id_kept BEFORE DELETE ON ledger_company WHEN EXISTS(SELECT 1 FROM attachments_attachment WHERE company_id=OLD.id) BEGIN SELECT RAISE(ABORT,'attachment_parent: Attachment parents cannot be deleted'); END;
CREATE TRIGGER attachment_operator_kept BEFORE DELETE ON fb_user WHEN EXISTS(SELECT 1 FROM attachments_attachment WHERE uploaded_by_id=OLD.id OR deleted_by_id=OLD.id) BEGIN SELECT RAISE(ABORT,'attachment_parent: Attachment operators cannot be deleted'); END;
