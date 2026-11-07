-- Keep permanent user identities for completion/undo, independently of display-name snapshots.
CREATE TABLE banking_reconciliationactor(
 reconciliation_id TEXT NOT NULL REFERENCES banking_reconciliation(id) DEFERRABLE INITIALLY DEFERRED,
 phase TEXT NOT NULL CHECK(phase IN('COMPLETE','UNDO')),
 user_id TEXT NOT NULL REFERENCES fb_user(id) DEFERRABLE INITIALLY DEFERRED,
 PRIMARY KEY(reconciliation_id,phase)
) WITHOUT ROWID, STRICT;
-- Real commands already recorded the exact actor in their append-only audit. Use
-- that identity first, so an operator rename cannot misassign a historical actor.
INSERT INTO banking_reconciliationactor
SELECT r.id,'COMPLETE',coalesce(
 (SELECT actor_id FROM ledger_auditevent e WHERE e.company_id=r.company_id AND e.target='reconciliation:'||r.id AND e.action='reconciliation.completed' ORDER BY e.id LIMIT 1),
 (SELECT id FROM fb_user WHERE username=json_extract(r.data,'$.completed_by')))
FROM banking_reconciliation r WHERE r.status IN('DONE','UNDONE');
INSERT INTO banking_reconciliationactor
SELECT r.id,'UNDO',coalesce(
 (SELECT actor_id FROM ledger_auditevent e WHERE e.company_id=r.company_id AND e.target='reconciliation:'||r.id AND e.action='reconciliation.undone' ORDER BY e.id LIMIT 1),
 (SELECT id FROM fb_user WHERE username=json_extract(r.data,'$.undone_by')))
FROM banking_reconciliation r WHERE r.status='UNDONE';
CREATE TRIGGER fb_banking_actor_insert BEFORE INSERT ON banking_reconciliationactor BEGIN
 SELECT RAISE(ABORT,'banking_actor_immutable: Reconciliation actors cannot be replaced') WHERE EXISTS(SELECT 1 FROM banking_reconciliationactor WHERE reconciliation_id=NEW.reconciliation_id AND phase=NEW.phase);
 SELECT RAISE(ABORT,'banking_actor_scope: Reconciliation actors require an eligible unsealed transition') WHERE NOT EXISTS(SELECT 1 FROM banking_reconciliation r JOIN fb_user u ON u.id=NEW.user_id WHERE r.id=NEW.reconciliation_id AND NOT EXISTS(SELECT 1 FROM banking_seal WHERE kind='reconciliation' AND doc_id=r.id||':'||r.revision) AND ((NEW.phase='COMPLETE' AND r.status='DONE' AND u.username=json_extract(r.data,'$.completed_by')) OR (NEW.phase='UNDO' AND r.status='UNDONE' AND u.username=json_extract(r.data,'$.undone_by'))));
END;
CREATE TRIGGER fb_banking_actor_primary_key_permanent BEFORE UPDATE OF reconciliation_id,phase ON banking_reconciliationactor
WHEN NEW.reconciliation_id IS NOT OLD.reconciliation_id OR NEW.phase IS NOT OLD.phase
BEGIN SELECT RAISE(ABORT,'primary_key_immutable: Primary keys never change'); END;
CREATE TRIGGER fb_banking_actor_update BEFORE UPDATE ON banking_reconciliationactor BEGIN SELECT RAISE(ABORT,'banking_actor_immutable: Reconciliation actors are immutable'); END;
CREATE TRIGGER fb_banking_actor_delete BEFORE DELETE ON banking_reconciliationactor BEGIN SELECT RAISE(ABORT,'banking_actor_immutable: Reconciliation actors are immutable'); END;
CREATE TRIGGER fb_banking_actor_user_kept BEFORE DELETE ON fb_user
WHEN EXISTS(SELECT 1 FROM banking_reconciliationactor WHERE user_id=OLD.id)
BEGIN SELECT RAISE(ABORT,'banking_parent_kept: Referenced banking parents cannot be deleted'); END;
CREATE VIEW banking_actor_invalid AS
SELECT r.id FROM banking_reconciliation r WHERE
 (r.status IN('DONE','UNDONE') AND NOT EXISTS(SELECT 1 FROM banking_reconciliationactor a JOIN fb_user u ON u.id=a.user_id WHERE a.reconciliation_id=r.id AND a.phase='COMPLETE')) OR
 (r.status='UNDONE' AND NOT EXISTS(SELECT 1 FROM banking_reconciliationactor a JOIN fb_user u ON u.id=a.user_id WHERE a.reconciliation_id=r.id AND a.phase='UNDO')) OR
 EXISTS(SELECT 1 FROM banking_reconciliationactor a WHERE a.reconciliation_id=r.id AND ((a.phase='COMPLETE' AND r.status='OPEN') OR (a.phase='UNDO' AND r.status<>'UNDONE')));
CREATE TRIGGER fb_banking_actor_seal BEFORE INSERT ON banking_seal WHEN NEW.kind='reconciliation' BEGIN
 SELECT RAISE(ABORT,'banking_actor_complete: Reconciliation transition requires permanent operator evidence') WHERE EXISTS(SELECT 1 FROM banking_reconciliation r JOIN banking_actor_invalid v ON v.id=r.id WHERE r.id||':'||r.revision=NEW.doc_id);
END;
-- Display labels follow the user FK; ownership is the permanent started_by_id and
-- actor rows. A legitimate rename must not obstruct a later Undo last.
DROP TRIGGER fb_banking_rec_update;
CREATE TRIGGER fb_banking_rec_update BEFORE UPDATE ON banking_reconciliation BEGIN
 SELECT RAISE(ABORT,'reconciliation_identity: A reconciliation keeps its company, account and origin') WHERE NEW.company_id IS NOT OLD.company_id OR NEW.account_id IS NOT OLD.account_id OR NEW.request_key IS NOT OLD.request_key OR NEW.creation_digest IS NOT OLD.creation_digest OR NEW.started_by_id IS NOT OLD.started_by_id;
 SELECT RAISE(ABORT,'reconciliation_revision: Reconciliation revision must step by one') WHERE OLD.revision=9223372036854775807 OR NEW.revision<>OLD.revision+1;
 SELECT RAISE(ABORT,'reconciliation_final: An undone reconciliation is final') WHERE OLD.status='UNDONE';
 SELECT RAISE(ABORT,'reconciliation_undo_open: Only a completed reconciliation can be undone') WHERE OLD.status='OPEN' AND NEW.status='UNDONE';
 SELECT RAISE(ABORT,'reconciliation_done_locked: A completed reconciliation changes only by Undo last') WHERE OLD.status='DONE' AND (NEW.status<>'UNDONE' OR NEW.statement_date IS NOT OLD.statement_date OR NEW.ending_balance IS NOT OLD.ending_balance OR NEW.hide_later IS NOT OLD.hide_later OR json_remove(NEW.data,'$.status','$.revision','$.undone_by','$.undo_key','$.undo_reason','$.started_by','$.completed_by') IS NOT json_remove(OLD.data,'$.status','$.revision','$.undone_by','$.undo_key','$.undo_reason','$.started_by','$.completed_by'));
 SELECT RAISE(ABORT,'reconciliation_undo_last: Only the most recent reconciliation of an account can be undone') WHERE OLD.status='DONE' AND EXISTS(SELECT 1 FROM banking_reconciliation r WHERE r.account_id=OLD.account_id AND r.status='DONE' AND r.id>OLD.id);
 SELECT RAISE(ABORT,'reconciliation_fields: Invalid reconciliation fields') WHERE json_extract(NEW.data,'$.id') IS NOT NEW.id OR json_extract(NEW.data,'$.company_id') IS NOT NEW.company_id OR json_extract(NEW.data,'$.account_id') IS NOT NEW.account_id OR json_extract(NEW.data,'$.request_key') IS NOT NEW.request_key OR json_extract(NEW.data,'$.creation_digest') IS NOT NEW.creation_digest OR json_extract(NEW.data,'$.revision') IS NOT NEW.revision OR json_extract(NEW.data,'$.status') IS NOT NEW.status OR json_extract(NEW.data,'$.statement_date') IS NOT NEW.statement_date OR fb_banking_money(json_quote(json_extract(NEW.data,'$.ending_balance'))) IS NOT NEW.ending_balance OR json_extract(NEW.data,'$.hide_later') IS NOT NEW.hide_later;
 SELECT RAISE(ABORT,'reconciliation_statement_order: The statement date is before the last reconciled statement') WHERE OLD.status='OPEN' AND EXISTS(SELECT 1 FROM banking_reconciliation WHERE account_id=NEW.account_id AND status='DONE' AND statement_date>NEW.statement_date);
 SELECT RAISE(ABORT,'reconciliation_beginning: The beginning balance must be the last reconciled ending balance') WHERE OLD.status='OPEN' AND NEW.status='DONE' AND fb_banking_money(json_quote(json_extract(NEW.data,'$.beginning_balance'))) IS NOT coalesce((SELECT ending_balance FROM banking_reconciliation WHERE account_id=NEW.account_id AND status='DONE' ORDER BY id DESC LIMIT 1),0);
END;

-- Preserve Python's debit-section rule after an unused designated account is reclassified.
DROP VIEW banking_charges_invalid;
CREATE VIEW banking_charges_invalid AS
SELECT r.id FROM banking_reconciliation r JOIN ledger_account a ON a.id=r.account_id
WHERE EXISTS(
 SELECT 1 FROM (SELECT 'service' prefix,'service_charge' amount,'EXPENSE' kind UNION ALL SELECT 'interest','interest','INCOME') x
 WHERE (json_extract(r.data,'$.'||x.amount) IS NULL AND (json_extract(r.data,'$.'||x.prefix||'_account_id') IS NOT NULL OR json_extract(r.data,'$.'||x.prefix||'_journal_id') IS NOT NULL))
 OR (json_extract(r.data,'$.'||x.amount) IS NOT NULL AND NOT EXISTS(
 SELECT 1 FROM ledger_journal j JOIN ledger_account b ON b.id=json_extract(r.data,'$.'||x.prefix||'_account_id')
 WHERE j.id=json_extract(r.data,'$.'||x.prefix||'_journal_id') AND j.company_id=r.company_id AND j.status='POSTED' AND j.source_id IS NULL AND j.reversal_of_id IS NULL
 AND b.company_id=r.company_id AND b.id<>a.id AND b.kind=x.kind
 AND fb_banking_money(json_quote(json_extract(r.data,'$.'||x.amount)))>0
 AND (x.prefix<>'interest' OR a.kind IN('ASSET','EXPENSE'))
 AND (SELECT count(*) FROM ledger_journalline WHERE journal_id=j.id)=2
 AND EXISTS(SELECT 1 FROM ledger_journalline l WHERE l.journal_id=j.id AND l.account_id=r.account_id AND l.debit=CASE WHEN x.prefix='interest' THEN fb_banking_money(json_quote(json_extract(r.data,'$.'||x.amount))) ELSE 0 END AND l.credit=CASE WHEN x.prefix='service' THEN fb_banking_money(json_quote(json_extract(r.data,'$.'||x.amount))) ELSE 0 END)
 AND EXISTS(SELECT 1 FROM ledger_journalline l WHERE l.journal_id=j.id AND l.account_id=b.id AND l.credit=CASE WHEN x.prefix='interest' THEN fb_banking_money(json_quote(json_extract(r.data,'$.'||x.amount))) ELSE 0 END AND l.debit=CASE WHEN x.prefix='service' THEN fb_banking_money(json_quote(json_extract(r.data,'$.'||x.amount))) ELSE 0 END)
 )));
