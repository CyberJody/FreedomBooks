-- Python's command includes every eligible C line; enforce the same complete set in SQL.
CREATE TRIGGER fb_banking_all_cleared_recorded BEFORE UPDATE ON banking_reconciliation
WHEN OLD.status='OPEN' AND NEW.status='DONE'
BEGIN
 SELECT RAISE(ABORT,'reconciliation_counted_set: Completion must record every eligible cleared line')
 WHERE EXISTS(SELECT 1 FROM banking_banklinestatus s JOIN ledger_journalline l ON l.id=s.journal_line_id JOIN ledger_journal j ON j.id=l.journal_id
  WHERE s.account_id=NEW.account_id AND s.company_id=NEW.company_id AND s.state='C' AND j.status='POSTED' AND (NEW.hide_later=0 OR j.date<=NEW.statement_date)
  AND NOT EXISTS(SELECT 1 FROM banking_reconciliationline x WHERE x.reconciliation_id=NEW.id AND x.journal_line_id=l.id));
END;
CREATE TRIGGER fb_banking_rec_request_key BEFORE INSERT ON banking_reconciliation
BEGIN
 SELECT RAISE(ABORT,'reconciliation_request_key: Reconciliation request keys must be canonical UUIDs')
 WHERE length(NEW.request_key)<>36 OR substr(NEW.request_key,9,1)<>'-' OR substr(NEW.request_key,14,1)<>'-' OR substr(NEW.request_key,19,1)<>'-' OR substr(NEW.request_key,24,1)<>'-' OR length(replace(NEW.request_key,'-',''))<>32 OR replace(NEW.request_key,'-','') GLOB '*[^0-9a-f]*';
END;
CREATE TRIGGER fb_banking_rec_transition_fields BEFORE UPDATE ON banking_reconciliation
BEGIN
 SELECT RAISE(ABORT,'reconciliation_transition_fields: Invalid completion or undo fields') WHERE
 (NEW.status='OPEN' AND (json_extract(NEW.data,'$.completion_key') IS NOT NULL OR json_extract(NEW.data,'$.beginning_balance') IS NOT NULL OR json_extract(NEW.data,'$.cleared_debit') IS NOT NULL OR json_extract(NEW.data,'$.cleared_credit') IS NOT NULL OR json_extract(NEW.data,'$.cleared_count') IS NOT NULL)) OR
 (NEW.status IN('DONE','UNDONE') AND (json_type(NEW.data,'$.completion_key') IS NOT 'text' OR length(json_extract(NEW.data,'$.completion_key'))<>36 OR json_type(NEW.data,'$.completed_by') IS NOT 'text' OR NOT EXISTS(SELECT 1 FROM fb_user WHERE username=json_extract(NEW.data,'$.completed_by')))) OR
 (NEW.status='UNDONE' AND (json_type(NEW.data,'$.undo_key') IS NOT 'text' OR length(json_extract(NEW.data,'$.undo_key'))<>36 OR json_type(NEW.data,'$.undone_by') IS NOT 'text' OR NOT EXISTS(SELECT 1 FROM fb_user WHERE username=json_extract(NEW.data,'$.undone_by')) OR length(trim(json_extract(NEW.data,'$.undo_reason'))) NOT BETWEEN 1 AND 500)) OR
 (NEW.status<>'UNDONE' AND (json_extract(NEW.data,'$.undo_key') IS NOT NULL OR coalesce(json_extract(NEW.data,'$.undo_reason'),'')<>''));
END;
