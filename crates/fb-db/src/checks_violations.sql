SELECT seed.id FROM checks_check seed WHERE NOT EXISTS(SELECT 1 FROM checks_checkrevision r JOIN checks_check c ON c.id=r.document_id WHERE seed.id||':'||seed.revision=c.id||':'||c.revision AND r.number=c.revision AND c.bank_id=r.bank_id AND c.vendor_id IS r.vendor_id AND c.state=r.state AND c.number_key=r.check_number AND (SELECT count(*) FROM checks_checkline WHERE revision_id=r.id) BETWEEN 1 AND 50 AND (SELECT min(position) FROM checks_checkline WHERE revision_id=r.id)=1 AND (SELECT max(position) FROM checks_checkline WHERE revision_id=r.id)=(SELECT count(*) FROM checks_checkline WHERE revision_id=r.id) AND r.total=(SELECT sum(amount) FROM checks_checkline WHERE revision_id=r.id))
UNION ALL
SELECT seed.id FROM checks_check seed WHERE EXISTS(SELECT 1 FROM checks_checkrevision r WHERE seed.id||':'||seed.revision=r.document_id||':'||r.number AND r.state IN('RECORDED','VOID') AND (
 EXISTS(SELECT position,tab,account_id,account_snapshot,item_id,item_snapshot,description,quantity,rate,amount FROM checks_checkline WHERE revision_id=r.id EXCEPT SELECT position,tab,account_id,account_snapshot,item_id,item_snapshot,description,quantity,rate,amount FROM checks_checkline WHERE revision_id=(SELECT id FROM checks_checkrevision WHERE document_id=r.document_id AND number=r.number-1)) OR
 EXISTS(SELECT position,tab,account_id,account_snapshot,item_id,item_snapshot,description,quantity,rate,amount FROM checks_checkline WHERE revision_id=(SELECT id FROM checks_checkrevision WHERE document_id=r.document_id AND number=r.number-1) EXCEPT SELECT position,tab,account_id,account_snapshot,item_id,item_snapshot,description,quantity,rate,amount FROM checks_checkline WHERE revision_id=r.id)))
UNION ALL
SELECT seed.id FROM checks_check seed WHERE EXISTS(SELECT 1 FROM checks_checkrevision r JOIN checks_check c ON c.id=r.document_id WHERE seed.id||':'||seed.revision=c.id||':'||r.number AND r.state IN('RECORDED','VOID') AND (
 NOT EXISTS(SELECT 1 FROM documents_documentaccount WHERE source_id=c.identity_id AND role='bank' AND account_id=r.bank_id AND account_kind='ASSET') OR
 EXISTS(SELECT 1 FROM checks_checkline l WHERE l.revision_id=r.id AND (l.account_id IS NULL OR l.account_id=r.bank_id OR NOT EXISTS(SELECT 1 FROM documents_documentaccount WHERE source_id=c.identity_id AND role='debit:'||l.position AND account_id=l.account_id))) OR
 (SELECT count(*) FROM documents_documentaccount WHERE source_id=c.identity_id)<>(SELECT count(*)+1 FROM checks_checkline WHERE revision_id=r.id) OR
 (SELECT count(*) FROM ledger_journalline WHERE journal_id=r.journal_id)<>(SELECT count(DISTINCT account_id)+1 FROM checks_checkline WHERE revision_id=r.id) OR
 NOT EXISTS(SELECT 1 FROM ledger_journalline WHERE journal_id=r.journal_id AND account_id=r.bank_id AND debit=0 AND credit=r.total) OR
 EXISTS(SELECT account_id,sum(amount),0 FROM checks_checkline WHERE revision_id=r.id GROUP BY account_id EXCEPT SELECT account_id,debit,credit FROM ledger_journalline WHERE journal_id=r.journal_id)))
UNION ALL
SELECT current_row.id FROM checks_checkrevision current_row WHERE (current_row.payee_kind='VENDOR')<>(current_row.vendor_id IS NOT NULL) OR trim(current_row.payee_name)='' OR current_row.check_number<>trim(current_row.check_number,char(32,9,10,13,12,11)) OR current_row.print_later=(current_row.check_number<>'')
UNION ALL
SELECT current_row.id FROM checks_checkrevision current_row WHERE (current_row.number=1 AND (current_row.state<>'DRAFT' OR current_row.action<>'created')) OR (current_row.number>1 AND NOT EXISTS(SELECT 1 FROM checks_checkrevision p WHERE p.document_id=current_row.document_id AND p.number=current_row.number-1 AND ((p.state='DRAFT' AND current_row.state='DRAFT' AND current_row.action='revised') OR (p.state='DRAFT' AND current_row.state='RECORDED' AND current_row.action='recorded') OR (p.state='RECORDED' AND current_row.state='RECORDED' AND current_row.action='numbered' AND p.print_later=1 AND current_row.print_later=0 AND current_row.check_number<>'') OR (p.state='RECORDED' AND current_row.state='VOID' AND current_row.action='voided'))))
UNION ALL
SELECT current_row.id FROM checks_checkrevision current_row WHERE current_row.state='VOID' AND trim(current_row.reason)=''
UNION ALL
SELECT current_row.id FROM checks_checkrevision current_row WHERE current_row.state IN('RECORDED','VOID') AND NOT EXISTS(SELECT 1 FROM checks_checkrevision p WHERE p.document_id=current_row.document_id AND p.number=current_row.number-1 AND p.bank_id IS current_row.bank_id AND p.bank_snapshot IS current_row.bank_snapshot AND p.payee_kind IS current_row.payee_kind AND p.vendor_id IS current_row.vendor_id AND p.payee_name IS current_row.payee_name AND p.payee_snapshot IS current_row.payee_snapshot AND p.address IS current_row.address AND p.company_snapshot IS current_row.company_snapshot AND p.date IS current_row.date AND p.memo IS current_row.memo AND p.total IS current_row.total AND (current_row.action='numbered' OR (p.check_number=current_row.check_number AND p.print_later=current_row.print_later)))
UNION ALL
SELECT t.id FROM transfers_transfer t WHERE (SELECT count(*) FROM ledger_journalline WHERE journal_id=t.journal_id)<>2 OR NOT EXISTS(SELECT 1 FROM ledger_journalline WHERE journal_id=t.journal_id AND account_id=t.from_account_id AND debit=0 AND credit=t.amount) OR NOT EXISTS(SELECT 1 FROM ledger_journalline WHERE journal_id=t.journal_id AND account_id=t.to_account_id AND debit=t.amount AND credit=0)
UNION ALL
SELECT t.id FROM transfers_transfer t WHERE (SELECT count(*) FROM documents_documentaccount WHERE source_id=t.identity_id)<>2 OR NOT EXISTS(SELECT 1 FROM documents_documentaccount WHERE source_id=t.identity_id AND role='from' AND account_id=t.from_account_id) OR NOT EXISTS(SELECT 1 FROM documents_documentaccount WHERE source_id=t.identity_id AND role='to' AND account_id=t.to_account_id)
UNION ALL
SELECT v.id FROM transfers_transfervoid v WHERE NOT EXISTS(SELECT 1 FROM transfers_transfer t JOIN ledger_journal j ON j.source_id=t.identity_id AND j.company_id=t.company_id AND j.reversal_of_id=t.journal_id WHERE t.id=v.transfer_id AND v.date>=t.date AND j.id=v.journal_id AND j.date=v.date AND j.status='POSTED')
UNION ALL
SELECT l.id FROM checks_checkline l WHERE fb_payables_line_valid(l.quantity,l.rate,l.amount)=0 OR l.amount<=0 OR l.rate<=0 OR (l.tab='EXPENSE' AND (l.item_id IS NOT NULL OR l.quantity<>10000)) OR (l.tab='ITEM' AND l.item_id IS NULL)
UNION ALL
SELECT c.id FROM checks_check c WHERE NOT EXISTS(SELECT 1 FROM checks_checkrevision r WHERE r.document_id=c.id AND r.number=c.revision AND r.state=c.state AND r.bank_id=c.bank_id AND r.vendor_id IS c.vendor_id AND r.check_number=c.number_key) OR NOT EXISTS(SELECT 1 FROM fb_seal WHERE kind='check_revision' AND doc_id=c.id||':'||c.revision)
UNION ALL
SELECT c.id FROM checks_check c JOIN fb_payables_live_checks p ON p.company_id=c.company_id AND p.bank_id=c.bank_id AND p.number=c.number_key WHERE c.state<>'VOID' AND c.number_key<>''
UNION ALL
SELECT t.id FROM transfers_transfer t WHERE NOT EXISTS(SELECT 1 FROM documents_documentidentity WHERE id=t.identity_id AND company_id=t.company_id AND kind='TRF') OR NOT EXISTS(SELECT 1 FROM ledger_journal WHERE id=t.journal_id AND company_id=t.company_id AND source_id=t.identity_id AND status='POSTED' AND reversal_of_id IS NULL AND date=t.date)
UNION ALL
SELECT c.id FROM checks_check c WHERE NOT EXISTS(SELECT 1 FROM documents_documentidentity WHERE id=c.identity_id AND company_id=c.company_id AND kind='CHK')
UNION ALL
SELECT l.id FROM checks_checkline l JOIN checks_checkrevision r ON r.id=l.revision_id JOIN checks_check c ON c.id=r.document_id WHERE (l.account_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account WHERE id=l.account_id AND company_id=c.company_id)) OR (l.item_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM directory_item WHERE id=l.item_id AND company_id=c.company_id))
UNION ALL
SELECT r.id FROM checks_checkrevision r JOIN checks_check c ON c.id=r.document_id WHERE NOT EXISTS(SELECT 1 FROM ledger_account WHERE id=r.bank_id AND company_id=c.company_id) OR (r.vendor_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM directory_vendor WHERE id=r.vendor_id AND company_id=c.company_id)) OR r.total<>(SELECT sum(amount) FROM checks_checkline WHERE revision_id=r.id) OR NOT EXISTS(SELECT 1 FROM fb_seal WHERE kind='check_revision' AND doc_id=c.id||':'||r.number)
UNION ALL
SELECT j.id FROM ledger_journal j JOIN documents_documentidentity i ON i.id=j.source_id WHERE i.kind='CHK' AND (j.status<>'POSTED' OR NOT (EXISTS(SELECT 1 FROM checks_check c JOIN checks_checkrevision r ON r.document_id=c.id AND r.number=c.revision WHERE c.identity_id=j.source_id AND r.state IN('RECORDED','VOID') AND ((j.reversal_of_id IS NULL AND r.journal_id=j.id) OR (r.state='VOID' AND r.journal_id=j.reversal_of_id AND r.reversal_id=j.id)))) OR NOT EXISTS(SELECT 1 FROM fb_seal WHERE kind='checks_journal' AND doc_id=j.id))
UNION ALL
SELECT j.id FROM ledger_journal j JOIN documents_documentidentity i ON i.id=j.source_id WHERE i.kind='TRF' AND (j.status<>'POSTED' OR NOT (EXISTS(SELECT 1 FROM transfers_transfer t WHERE t.identity_id=j.source_id AND ((j.reversal_of_id IS NULL AND t.journal_id=j.id) OR (t.journal_id=j.reversal_of_id AND EXISTS(SELECT 1 FROM transfers_transfervoid v WHERE v.transfer_id=t.id AND v.journal_id=j.id))))) OR NOT EXISTS(SELECT 1 FROM fb_seal WHERE kind='transfer_journal' AND doc_id=j.id))

UNION ALL
SELECT current_row.id FROM checks_checkrevision current_row WHERE (current_row.state='DRAFT' AND (current_row.journal_id IS NOT NULL OR current_row.reversal_id IS NOT NULL OR current_row.review_digest<>'')) OR
 (current_row.state IN('RECORDED','VOID') AND (length(current_row.review_digest)<>64 OR NOT EXISTS(SELECT 1 FROM ledger_journal j JOIN checks_check c ON c.identity_id=j.source_id WHERE c.id=current_row.document_id AND j.id=current_row.journal_id AND j.company_id=c.company_id AND j.status='POSTED' AND j.reversal_of_id IS NULL AND j.date=current_row.date))) OR
 (current_row.state='RECORDED' AND current_row.reversal_id IS NOT NULL) OR
 (current_row.action IN('numbered','voided') AND NOT EXISTS(SELECT 1 FROM checks_checkrevision p WHERE p.document_id=current_row.document_id AND p.number=current_row.number-1 AND p.journal_id=current_row.journal_id AND p.review_digest=current_row.review_digest)) OR
 (current_row.state='VOID' AND NOT EXISTS(SELECT 1 FROM ledger_journal j JOIN checks_check c ON c.identity_id=j.source_id WHERE c.id=current_row.document_id AND j.id=current_row.reversal_id AND j.reversal_of_id=current_row.journal_id AND j.status='POSTED' AND j.date>=current_row.date))

UNION ALL
SELECT r.id FROM checks_checkrevision r JOIN checks_check c ON c.id=r.document_id WHERE fb_checks_snapshot_valid('account',r.bank_snapshot,r.bank_id)=0
 OR fb_checks_snapshot_valid('party',r.payee_snapshot,r.vendor_id)=0
 OR fb_checks_snapshot_valid('company',r.company_snapshot,c.company_id)=0
 OR fb_check_number_valid(r.check_number)=0
UNION ALL SELECT l.id FROM checks_checkline l WHERE fb_checks_snapshot_valid('account',l.account_snapshot,l.account_id)=0
 OR fb_checks_snapshot_valid('item',l.item_snapshot,l.item_id)=0
UNION ALL SELECT t.id FROM transfers_transfer t WHERE fb_checks_snapshot_valid('account',t.from_snapshot,t.from_account_id)=0
 OR fb_checks_snapshot_valid('account',t.to_snapshot,t.to_account_id)=0
 OR fb_checks_snapshot_valid('company',t.company_snapshot,t.company_id)=0
UNION ALL SELECT c.id FROM checks_check c WHERE fb_check_number_valid(c.number_key)=0
UNION ALL SELECT l.id FROM checks_checkline l JOIN checks_checkrevision r ON r.id=l.revision_id WHERE l.origin_position IS NOT NULL AND NOT EXISTS(
 SELECT 1 FROM checks_checkrevision p JOIN checks_checkline old ON old.revision_id=p.id
 WHERE p.document_id=r.document_id AND p.number=r.number-1 AND old.position=l.origin_position)
UNION ALL SELECT min(id) FROM checks_checkline WHERE origin_position IS NOT NULL GROUP BY revision_id,origin_position HAVING count(*)>1
