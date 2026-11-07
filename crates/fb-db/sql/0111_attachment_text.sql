-- Reject NUL in every attachment text column, with byte-exact identifiers and strict UTC time.
CREATE VIEW attachment_text_invalid AS SELECT id FROM attachments_attachment WHERE instr(id,char(0))>0 OR
 instr(company_id,char(0))>0 OR
 instr(document_id,char(0))>0 OR
 instr(journal_id,char(0))>0 OR
 instr(original_name,char(0))>0 OR
 instr(stored_name,char(0))>0 OR
 instr(content_type,char(0))>0 OR
 instr(sha256,char(0))>0 OR
 instr(original_sha256,char(0))>0 OR
 instr(wrapped_key,char(0))>0 OR
 instr(wrap_nonce,char(0))>0 OR
 instr(nonce,char(0))>0 OR
 instr(uploaded_by_id,char(0))>0 OR
 instr(uploaded_by_name,char(0))>0 OR
 instr(uploaded_at,char(0))>0 OR
 instr(uploaded_number,char(0))>0 OR
 instr(deleted_number,char(0))>0 OR
 instr(request_key,char(0))>0 OR
 instr(deleted_by_id,char(0))>0 OR
 instr(deleted_by_name,char(0))>0 OR
 instr(deleted_at,char(0))>0 OR
 instr(delete_reason,char(0))>0 OR
 NOT fb_attachment_uuid(id) OR NOT fb_attachment_uuid(request_key) OR
 NOT fb_attachment_timestamp(uploaded_at) OR
 (deleted_at IS NOT NULL AND NOT fb_attachment_timestamp(deleted_at));
CREATE TRIGGER attachment_text_insert BEFORE INSERT ON attachments_attachment BEGIN
 SELECT RAISE(ABORT,'attachment_text: Invalid attachment text or timestamp') WHERE instr(NEW.id,char(0))>0 OR
 instr(NEW.company_id,char(0))>0 OR
 instr(NEW.document_id,char(0))>0 OR
 instr(NEW.journal_id,char(0))>0 OR
 instr(NEW.original_name,char(0))>0 OR
 instr(NEW.stored_name,char(0))>0 OR
 instr(NEW.content_type,char(0))>0 OR
 instr(NEW.sha256,char(0))>0 OR
 instr(NEW.original_sha256,char(0))>0 OR
 instr(NEW.wrapped_key,char(0))>0 OR
 instr(NEW.wrap_nonce,char(0))>0 OR
 instr(NEW.nonce,char(0))>0 OR
 instr(NEW.uploaded_by_id,char(0))>0 OR
 instr(NEW.uploaded_by_name,char(0))>0 OR
 instr(NEW.uploaded_at,char(0))>0 OR
 instr(NEW.uploaded_number,char(0))>0 OR
 instr(NEW.deleted_number,char(0))>0 OR
 instr(NEW.request_key,char(0))>0 OR
 instr(NEW.deleted_by_id,char(0))>0 OR
 instr(NEW.deleted_by_name,char(0))>0 OR
 instr(NEW.deleted_at,char(0))>0 OR
 instr(NEW.delete_reason,char(0))>0 OR
 NOT fb_attachment_uuid(NEW.id) OR NOT fb_attachment_uuid(NEW.request_key) OR
 NOT fb_attachment_timestamp(NEW.uploaded_at) OR
 (NEW.deleted_at IS NOT NULL AND NOT fb_attachment_timestamp(NEW.deleted_at));
END;
CREATE TRIGGER attachment_text_update BEFORE UPDATE ON attachments_attachment BEGIN
 SELECT RAISE(ABORT,'attachment_text: Invalid attachment text or timestamp') WHERE instr(NEW.id,char(0))>0 OR
 instr(NEW.company_id,char(0))>0 OR
 instr(NEW.document_id,char(0))>0 OR
 instr(NEW.journal_id,char(0))>0 OR
 instr(NEW.original_name,char(0))>0 OR
 instr(NEW.stored_name,char(0))>0 OR
 instr(NEW.content_type,char(0))>0 OR
 instr(NEW.sha256,char(0))>0 OR
 instr(NEW.original_sha256,char(0))>0 OR
 instr(NEW.wrapped_key,char(0))>0 OR
 instr(NEW.wrap_nonce,char(0))>0 OR
 instr(NEW.nonce,char(0))>0 OR
 instr(NEW.uploaded_by_id,char(0))>0 OR
 instr(NEW.uploaded_by_name,char(0))>0 OR
 instr(NEW.uploaded_at,char(0))>0 OR
 instr(NEW.uploaded_number,char(0))>0 OR
 instr(NEW.deleted_number,char(0))>0 OR
 instr(NEW.request_key,char(0))>0 OR
 instr(NEW.deleted_by_id,char(0))>0 OR
 instr(NEW.deleted_by_name,char(0))>0 OR
 instr(NEW.deleted_at,char(0))>0 OR
 instr(NEW.delete_reason,char(0))>0 OR
 NOT fb_attachment_uuid(NEW.id) OR NOT fb_attachment_uuid(NEW.request_key) OR
 NOT fb_attachment_timestamp(NEW.uploaded_at) OR
 (NEW.deleted_at IS NOT NULL AND NOT fb_attachment_timestamp(NEW.deleted_at));
END;
