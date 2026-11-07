-- Match the configured roles when posting a NEW credit. Frozen historical
-- mappings remain valid across subsequent accounting-setup changes.
CREATE TRIGGER fb_credits_memo_designations BEFORE INSERT ON credits_creditmemo
WHEN NOT EXISTS(SELECT 1 FROM credits_creditmemo WHERE id=NEW.id OR identity_id=NEW.identity_id OR journal_id=NEW.journal_id)
BEGIN
 SELECT RAISE(ABORT,'credit_memo_designation: Choose configured receivables and collected tax')
 WHERE NOT EXISTS(SELECT 1 FROM documents_accountingsetup s
   WHERE s.company_id=NEW.company_id AND s.receivables_id=NEW.receivables_id
     AND (NEW.collected_tax_id IS NULL OR s.collected_tax_id=NEW.collected_tax_id));
END;

-- The pure Rust field validator checks every nested shape and its frozen IDs.
-- Insertion guards in 0060 already call it; sealing and open scan it again.
CREATE VIEW credits_snapshot_violations AS
 SELECT 'creditmemo snapshot: '||id problem FROM credits_creditmemo WHERE NOT fb_credits_fields('creditmemo',data)
 UNION ALL
 SELECT 'creditmemoline snapshot: '||id problem FROM credits_creditmemoline WHERE NOT fb_credits_fields('creditmemoline',data)
 UNION ALL
 SELECT 'creditmemovoid snapshot: '||id problem FROM credits_creditmemovoid WHERE NOT fb_credits_fields('creditmemovoid',data)
 UNION ALL
 SELECT 'creditapplication snapshot: '||id problem FROM credits_creditapplication WHERE NOT fb_credits_fields('creditapplication',data)
 UNION ALL
 SELECT 'creditapplicationline snapshot: '||id problem FROM credits_creditapplicationline WHERE NOT fb_credits_fields('creditapplicationline',data)
 UNION ALL
 SELECT 'creditunapplication snapshot: '||id problem FROM credits_creditunapplication WHERE NOT fb_credits_fields('creditunapplication',data)
 UNION ALL
 SELECT 'creditrefund snapshot: '||id problem FROM credits_creditrefund WHERE NOT fb_credits_fields('creditrefund',data)
 UNION ALL
 SELECT 'creditrefundvoid snapshot: '||id problem FROM credits_creditrefundvoid WHERE NOT fb_credits_fields('creditrefundvoid',data);
CREATE TRIGGER fb_credits_snapshot_seal BEFORE INSERT ON credits_seal
BEGIN
 SELECT RAISE(ABORT,'credit_snapshot: Invalid stored credit snapshots')
 WHERE EXISTS(SELECT 1 FROM credits_snapshot_violations);
END;
