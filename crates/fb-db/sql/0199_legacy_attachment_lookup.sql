-- Routine reads on modern books must not rescan their whole audit history looking for legacy wraps.
-- New re-wrap events contain file-name strings; only the historical object-array form enters this index.
CREATE INDEX attachment_legacy_keywrap_lookup ON ledger_auditevent(id DESC)
 WHERE action='attachment.key_rewrapped' AND json_type(detail,'$.files[0]')='object';
