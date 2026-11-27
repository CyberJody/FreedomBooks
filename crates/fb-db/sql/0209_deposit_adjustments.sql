-- Worker item 47 (2026-11-24): a deposit may carry fee and adjustment lines, for a card-processor payout
-- that arrives net of its fee. They are part of the deposit's frozen payload, `data.adjustments`:
-- [{"account_id": <account>, "amount": <signed money>}], one per account, a fee negative. The bank gets the
-- receipts plus the adjustments (an 850.00 payment less a 24.95 fee deposits 825.05); each adjustment posts
-- to its own account, a negative one as a debit and a positive one as a credit, and has its own account
-- evidence (role 'adjustment:<account>'). The payload check (`fb_banking_payload`) refuses malformed lines.
-- A deposit without adjustments is checked exactly as before.
DROP VIEW banking_deposit_invalid;
DROP VIEW banking_deposit_expected;
CREATE VIEW banking_deposit_adjustment AS
 SELECT d.id deposit_id,json_extract(a.value,'$.account_id') account_id,fb_banking_money(json_quote(json_extract(a.value,'$.amount'))) amount
 FROM banking_deposit d JOIN json_each(d.data,'$.adjustments') a;
CREATE VIEW banking_deposit_expected AS
 SELECT d.id deposit_id,d.bank_id account_id,d.amount debit,0 credit FROM banking_deposit d
 UNION ALL SELECT l.deposit_id,l.undeposited_id,0,fb_banking_sum(json_group_array(l.amount)) FROM banking_depositline l GROUP BY l.deposit_id,l.undeposited_id
 UNION ALL SELECT a.deposit_id,a.account_id,CASE WHEN a.amount<0 THEN -a.amount ELSE 0 END,CASE WHEN a.amount>0 THEN a.amount ELSE 0 END FROM banking_deposit_adjustment a;
CREATE VIEW banking_deposit_invalid AS SELECT d.id FROM banking_deposit d WHERE
 (SELECT count(*) FROM banking_depositline WHERE deposit_id=d.id) NOT BETWEEN 1 AND 50 OR
 EXISTS(SELECT 1 FROM banking_deposit_adjustment a WHERE a.deposit_id=d.id AND (a.account_id IS NULL OR a.amount IS NULL OR a.amount=0 OR a.account_id=d.bank_id
  OR EXISTS(SELECT 1 FROM banking_depositline l WHERE l.deposit_id=d.id AND l.undeposited_id=a.account_id)
  OR NOT EXISTS(SELECT 1 FROM documents_documentaccount x WHERE x.source_id=d.identity_id AND x.role='adjustment:'||a.account_id AND x.account_id=a.account_id))) OR
 (SELECT count(*) FROM banking_deposit_adjustment WHERE deposit_id=d.id)<>(SELECT count(DISTINCT account_id) FROM banking_deposit_adjustment WHERE deposit_id=d.id) OR
 d.amount<>fb_banking_sum((SELECT json_group_array(amount) FROM (SELECT amount FROM banking_depositline WHERE deposit_id=d.id UNION ALL SELECT amount FROM banking_deposit_adjustment WHERE deposit_id=d.id))) OR
 EXISTS(SELECT account_id,debit,credit FROM banking_deposit_expected WHERE deposit_id=d.id EXCEPT SELECT account_id,debit,credit FROM ledger_journalline WHERE journal_id=d.journal_id) OR
 EXISTS(SELECT account_id,debit,credit FROM ledger_journalline WHERE journal_id=d.journal_id EXCEPT SELECT account_id,debit,credit FROM banking_deposit_expected WHERE deposit_id=d.id) OR
 (SELECT count(*) FROM ledger_journalline WHERE journal_id=d.journal_id)<>(SELECT count(*) FROM banking_deposit_expected WHERE deposit_id=d.id) OR
 EXISTS(SELECT 1 FROM banking_depositline l WHERE l.deposit_id=d.id AND NOT EXISTS(SELECT 1 FROM documents_documentaccount a WHERE a.source_id=d.identity_id AND a.role='undeposited:'||l.undeposited_id AND a.account_id=l.undeposited_id)) OR
 NOT EXISTS(SELECT 1 FROM documents_documentaccount a WHERE a.source_id=d.identity_id AND a.role='bank' AND a.account_id=d.bank_id) OR
 (SELECT count(*) FROM documents_documentaccount a WHERE a.source_id=d.identity_id)<>(SELECT count(*) FROM banking_deposit_expected WHERE deposit_id=d.id);
