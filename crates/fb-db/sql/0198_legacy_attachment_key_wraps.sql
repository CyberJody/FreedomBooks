-- Preserve authentic books produced before the attachment key-wrap table existed. Their audit rows
-- are append-only evidence and can contain key wraps, including files of other companies. Admit only
-- that historical shape; the strict 0197 insert trigger still refuses any new event in this form.
CREATE VIEW attachment_legacy_keywrap_events AS
 SELECT e.id FROM ledger_auditevent e WHERE e.action='attachment.key_rewrapped'
 AND json_valid(e.detail) AND json_type(e.detail)='object'
 AND (SELECT count(*) FROM json_each(e.detail))=2
 AND json_type(e.detail,'$.attachments')='array' AND json_array_length(e.detail,'$.attachments')>0
 AND json_type(e.detail,'$.files')='array' AND json_array_length(e.detail,'$.files')>0
 AND NOT EXISTS(SELECT 1 FROM json_each(e.detail,'$.attachments') a WHERE a.type<>'text'
     OR NOT EXISTS(SELECT 1 FROM attachments_attachment r WHERE r.id=a.value
         AND EXISTS(SELECT 1 FROM json_each(e.detail,'$.files') f WHERE f.type='object' AND json_extract(CASE WHEN f.type='object' THEN f.value ELSE '{}' END,'$.stored_name')=r.stored_name)))
 AND NOT EXISTS(SELECT 1 FROM json_each(e.detail,'$.files') f WHERE f.type<>'object'
     OR (SELECT count(*) FROM json_each(CASE WHEN f.type='object' THEN f.value ELSE '{}' END))<>3
     OR json_type(CASE WHEN f.type='object' THEN f.value ELSE '{}' END,'$.stored_name') IS NOT 'text' OR json_type(CASE WHEN f.type='object' THEN f.value ELSE '{}' END,'$.wrapped_key') IS NOT 'text' OR json_type(CASE WHEN f.type='object' THEN f.value ELSE '{}' END,'$.wrap_nonce') IS NOT 'text'
     OR length(json_extract(CASE WHEN f.type='object' THEN f.value ELSE '{}' END,'$.stored_name'))<>36 OR substr(json_extract(CASE WHEN f.type='object' THEN f.value ELSE '{}' END,'$.stored_name'),33)<>'.bin' OR substr(json_extract(CASE WHEN f.type='object' THEN f.value ELSE '{}' END,'$.stored_name'),1,32) GLOB '*[^0-9a-f]*'
     OR length(json_extract(CASE WHEN f.type='object' THEN f.value ELSE '{}' END,'$.wrapped_key'))<>96 OR json_extract(CASE WHEN f.type='object' THEN f.value ELSE '{}' END,'$.wrapped_key') GLOB '*[^0-9a-f]*'
     OR length(json_extract(CASE WHEN f.type='object' THEN f.value ELSE '{}' END,'$.wrap_nonce'))<>24 OR json_extract(CASE WHEN f.type='object' THEN f.value ELSE '{}' END,'$.wrap_nonce') GLOB '*[^0-9a-f]*'
     OR NOT EXISTS(SELECT 1 FROM attachments_attachment r WHERE r.stored_name=json_extract(CASE WHEN f.type='object' THEN f.value ELSE '{}' END,'$.stored_name')
         AND EXISTS(SELECT 1 FROM json_each(e.detail,'$.attachments') a WHERE a.type='text' AND a.value=r.id)));

DROP VIEW attachment_keywrap_invalid;
CREATE VIEW attachment_keywrap_invalid AS
 SELECT w.company_id||':'||w.stored_name||':'||w.position AS problem FROM attachments_keywrap w LEFT JOIN ledger_auditevent e ON e.id=w.audit_id
 WHERE NOT EXISTS(SELECT 1 FROM attachments_attachment a WHERE a.company_id=w.company_id AND a.stored_name=w.stored_name)
 OR e.id IS NULL OR e.company_id IS NOT w.company_id OR e.action IS NOT 'attachment.key_rewrapped'
 OR NOT EXISTS(SELECT 1 FROM json_each(e.detail,'$.files') f WHERE f.type='text' AND f.value=w.stored_name)
 OR w.position<>(SELECT count(*) FROM attachments_keywrap v WHERE v.company_id=w.company_id AND v.stored_name=w.stored_name AND v.position<=w.position)
 OR EXISTS(SELECT 1 FROM attachments_attachment a WHERE a.company_id=w.company_id AND a.stored_name=w.stored_name AND (a.wrap_nonce=w.wrap_nonce OR a.wrapped_key=w.wrapped_key))
 OR EXISTS(SELECT 1 FROM attachments_keywrap v WHERE v.company_id=w.company_id AND v.stored_name=w.stored_name AND v.position<>w.position AND v.wrapped_key=w.wrapped_key)
 UNION ALL
 SELECT 'event:'||e.id FROM ledger_auditevent e WHERE e.action='attachment.key_rewrapped' AND NOT EXISTS(SELECT 1 FROM attachment_legacy_keywrap_events l WHERE l.id=e.id) AND (NOT json_valid(e.detail) OR json_type(e.detail) IS NOT 'object'
 OR (SELECT count(*) FROM json_each(e.detail))<>2 OR json_type(e.detail,'$.attachments') IS NOT 'array' OR json_type(e.detail,'$.files') IS NOT 'array'
 OR json_array_length(e.detail,'$.attachments')=0 OR json_array_length(e.detail,'$.files')=0
 OR EXISTS(SELECT 1 FROM json_each(e.detail,'$.attachments') j WHERE j.type<>'text' OR NOT EXISTS(SELECT 1 FROM attachments_attachment a WHERE a.id=j.value AND a.company_id=e.company_id))
 OR EXISTS(SELECT 1 FROM json_each(e.detail,'$.files') f WHERE f.type<>'text' OR NOT EXISTS(SELECT 1 FROM attachments_attachment a WHERE a.stored_name=f.value AND a.company_id=e.company_id)));
