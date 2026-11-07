-- L1-FIX2: remove hidden rowids and refuse UPDATE replacement victims.
-- Preserve all data with TEMP copies; all FKs are deferred until this migration commits.
-- Rebuild in one IMMEDIATE transaction, with guards restored before committing.
CREATE TEMP TABLE copy_fb_user AS SELECT * FROM main.fb_user;
CREATE TEMP TABLE copy_ledger_company AS SELECT * FROM main.ledger_company;
CREATE TEMP TABLE copy_ledger_company_members AS SELECT * FROM main.ledger_company_members;
CREATE TEMP TABLE copy_ledger_account AS SELECT * FROM main.ledger_account;
CREATE TEMP TABLE copy_ledger_auditevent AS SELECT * FROM main.ledger_auditevent;
CREATE TEMP TABLE copy_ledger_journal AS SELECT * FROM main.ledger_journal;
CREATE TEMP TABLE copy_ledger_journalline AS SELECT * FROM main.ledger_journalline;
CREATE TEMP TABLE copy_fb_seal AS SELECT * FROM main.fb_seal;
CREATE TEMP TABLE copy_fb_seal_due AS SELECT * FROM main.fb_seal_due;
DROP TRIGGER fb_seal_frozen;
DROP TRIGGER fb_seal_kept;
DROP TRIGGER fb_seal_due_frozen;
DROP TRIGGER fb_seal_due_kept;
DROP TRIGGER fb_company_guard_insert;
DROP TRIGGER fb_company_guard_update;
DROP TRIGGER fb_account_guard_insert;
DROP TRIGGER fb_account_guard_update;
DROP TRIGGER fb_account_fill_type_insert;
DROP TRIGGER fb_account_fill_type_update;
DROP TRIGGER fb_audit_no_update;
DROP TRIGGER fb_audit_no_delete;
DROP TRIGGER fb_journal_guard_insert;
DROP TRIGGER fb_journal_guard_update;
DROP TRIGGER fb_journal_guard_delete;
DROP TRIGGER fb_line_guard_insert;
DROP TRIGGER fb_line_guard_update;
DROP TRIGGER fb_line_guard_delete;
DROP TRIGGER fb_company_guard_delete;
DROP TRIGGER fb_account_guard_delete;
DROP TRIGGER fb_audit_guard_insert;
DROP TRIGGER fb_user_guard_insert;
DROP TRIGGER fb_user_guard_delete;
DROP TRIGGER fb_seal_guard_insert;
DROP TRIGGER fb_seal_due_guard_insert;
DROP TABLE fb_seal_due;
DROP TABLE ledger_journalline;
DROP TABLE ledger_journal;
DROP TABLE ledger_auditevent;
DROP TABLE ledger_company_members;
DROP TABLE ledger_account;
DROP TABLE ledger_company;
DROP TABLE fb_user;
DROP TABLE fb_seal;

-- FreedomBooks book schema, migration 0001: the ledger (layer 1).
-- Port of the Python app's ledger tables and their PostgreSQL guards (ledger migrations 0001-0012 and
-- documents/0002 fb_company_guard), per design/SAFETY-RULES-PORT.md:
--   * ids are UUIDv7 TEXT; money is INTEGER cents in STRICT tables (no REAL can land);
--   * every guard message is RAISE(ABORT, '<constraint or function name>: <the PostgreSQL message>');
--   * one trigger per table/event so no rule depends on firing order (SQLite has none; probe F11);
--   * money arithmetic stays in Rust: the triggers only compare integer sums (sum() raises on overflow).
-- Not here yet (layer 2): source documents (source_id, fb_source_journal_guard, the *_journal_owner seals),
-- control accounts (fb_journal_zcontrol), cleared bank lines (fb_reversal_cleared_guard), and the
-- document-identity half of fb_company_guard.

CREATE TABLE fb_user (
  id           TEXT PRIMARY KEY CHECK (length(id) = 36),
  username     TEXT NOT NULL UNIQUE CHECK (length(username) BETWEEN 1 AND 150),
  is_active    INTEGER NOT NULL DEFAULT 1 CHECK (is_active IN (0, 1)),
  is_staff     INTEGER NOT NULL DEFAULT 0 CHECK (is_staff IN (0, 1)),
  is_superuser INTEGER NOT NULL DEFAULT 0 CHECK (is_superuser IN (0, 1))
) STRICT, WITHOUT ROWID;

CREATE TABLE ledger_company (
  id                 TEXT PRIMARY KEY CHECK (length(id) = 36),
  name               TEXT NOT NULL CHECK (length(name) <= 120),
  currency           TEXT NOT NULL DEFAULT 'USD' CHECK (length(currency) = 3 AND currency NOT GLOB '*[^A-Z]*'),
  first_run_open     INTEGER NOT NULL DEFAULT 0 CHECK (first_run_open IN (0, 1)),
  bill_reminder_days INTEGER NOT NULL DEFAULT 10
                     CONSTRAINT ledger_company_bill_reminder_days_0_90 CHECK (bill_reminder_days BETWEEN 0 AND 90)
) STRICT, WITHOUT ROWID;

CREATE TABLE ledger_company_members (
  company_id TEXT NOT NULL REFERENCES ledger_company (id) DEFERRABLE INITIALLY DEFERRED,
  user_id    TEXT NOT NULL REFERENCES fb_user (id) DEFERRABLE INITIALLY DEFERRED,
  PRIMARY KEY (company_id, user_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE ledger_account (
  id          TEXT PRIMARY KEY CHECK (length(id) = 36),
  company_id  TEXT NOT NULL REFERENCES ledger_company (id) DEFERRABLE INITIALLY DEFERRED,
  code        TEXT NOT NULL CHECK (length(code) <= 20),
  name        TEXT NOT NULL CHECK (length(name) <= 120),
  kind        TEXT NOT NULL CHECK (kind IN ('ASSET', 'LIABILITY', 'EQUITY', 'INCOME', 'EXPENSE')),
  -- '' = "the plain type of the kind": filled by fb_account_fill_type_* (PG fills it in fb_account_guard).
  detail_type TEXT NOT NULL DEFAULT '',
  is_contra   INTEGER NOT NULL DEFAULT 0 CHECK (is_contra IN (0, 1)),
  description TEXT NOT NULL DEFAULT '' CHECK (length(description) <= 200),
  active      INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1)),
  postable    INTEGER NOT NULL DEFAULT 1 CHECK (postable IN (0, 1)),
  CONSTRAINT account_detail_type_kind CHECK (
       detail_type = ''
    OR (kind = 'ASSET' AND detail_type IN ('BANK', 'RECEIVABLE', 'OTHER_CURRENT_ASSET', 'FIXED_ASSET', 'OTHER_ASSET'))
    OR (kind = 'LIABILITY' AND detail_type IN ('PAYABLE', 'CREDIT_CARD', 'OTHER_CURRENT_LIABILITY', 'LONG_TERM_LIABILITY'))
    OR (kind = 'EQUITY' AND detail_type IN ('EQUITY'))
    OR (kind = 'INCOME' AND detail_type IN ('INCOME', 'OTHER_INCOME'))
    OR (kind = 'EXPENSE' AND detail_type IN ('COST_OF_GOODS_SOLD', 'EXPENSE', 'OTHER_EXPENSE'))),
  CONSTRAINT account_contra_type CHECK (NOT (is_contra = 1 AND detail_type IN ('BANK', 'RECEIVABLE', 'PAYABLE', 'CREDIT_CARD'))),
  CONSTRAINT account_code_per_company UNIQUE (company_id, code)
) STRICT, WITHOUT ROWID;

CREATE TABLE ledger_auditevent (
  id          TEXT PRIMARY KEY CHECK (length(id) = 36),
  company_id  TEXT NOT NULL REFERENCES ledger_company (id) DEFERRABLE INITIALLY DEFERRED,
  actor_id    TEXT NOT NULL REFERENCES fb_user (id) DEFERRABLE INITIALLY DEFERRED,
  actor_name  TEXT NOT NULL CHECK (length(actor_name) <= 150),
  occurred_at TEXT NOT NULL,
  action      TEXT NOT NULL CHECK (length(action) <= 40),
  target      TEXT NOT NULL CHECK (length(target) <= 100),
  detail      TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(detail))
) STRICT, WITHOUT ROWID;
CREATE INDEX ledger_auditevent_company ON ledger_auditevent (company_id, occurred_at);

CREATE TABLE ledger_journal (
  id              TEXT PRIMARY KEY CHECK (length(id) = 36),
  -- The J-number shown for document journals and reversals (Python: f"J{pk:06d}" of the serial pk).
  -- UUID ids have no human number, so the book keeps one sequence across companies, as the pk was.
  number          INTEGER NOT NULL UNIQUE CHECK (number >= 1),
  company_id      TEXT NOT NULL REFERENCES ledger_company (id) DEFERRABLE INITIALLY DEFERRED,
  date            TEXT NOT NULL CHECK (date(date) IS date),
  memo            TEXT NOT NULL CHECK (length(memo) <= 240),
  status          TEXT NOT NULL DEFAULT 'DRAFT',
  created_by_id   TEXT NOT NULL REFERENCES fb_user (id) DEFERRABLE INITIALLY DEFERRED,
  created_at      TEXT NOT NULL,
  posted_by_id    TEXT REFERENCES fb_user (id) DEFERRABLE INITIALLY DEFERRED,
  posted_at       TEXT,
  reversal_of_id  TEXT UNIQUE REFERENCES ledger_journal (id) DEFERRABLE INITIALLY DEFERRED,
  entry_number    TEXT NOT NULL DEFAULT '' CHECK (length(entry_number) <= 20),
  adjusting       INTEGER NOT NULL DEFAULT 0 CHECK (adjusting IN (0, 1)),
  request_key     TEXT CHECK (request_key IS NULL OR length(request_key) = 36),
  creation_digest TEXT NOT NULL DEFAULT '' CHECK (length(creation_digest) <= 64),
  revision        INTEGER NOT NULL DEFAULT 1 CONSTRAINT ledger_journal_revision_check CHECK (revision >= 0)
) STRICT, WITHOUT ROWID;
CREATE UNIQUE INDEX journal_entry_number_per_company ON ledger_journal (company_id, entry_number) WHERE entry_number <> '';
CREATE UNIQUE INDEX journal_request_key_per_company ON ledger_journal (company_id, request_key) WHERE request_key IS NOT NULL;
CREATE INDEX ledger_journal_company_date ON ledger_journal (company_id, date);

CREATE TABLE ledger_journalline (
  id          TEXT PRIMARY KEY CHECK (length(id) = 36),
  journal_id  TEXT NOT NULL REFERENCES ledger_journal (id) DEFERRABLE INITIALLY DEFERRED,
  account_id  TEXT NOT NULL REFERENCES ledger_account (id) DEFERRABLE INITIALLY DEFERRED,
  position    INTEGER NOT NULL CONSTRAINT ledger_journalline_position_check CHECK (position >= 0),
  debit       INTEGER NOT NULL DEFAULT 0 CHECK (debit BETWEEN 0 AND 9999999999999999),
  credit      INTEGER NOT NULL DEFAULT 0 CHECK (credit BETWEEN 0 AND 9999999999999999),
  description TEXT NOT NULL DEFAULT '' CHECK (length(description) <= 160),
  name        TEXT NOT NULL DEFAULT '' CHECK (length(name) <= 120),
  CONSTRAINT one_positive_side CHECK ((debit > 0 AND credit = 0) OR (credit > 0 AND debit = 0)),
  CONSTRAINT unique_journal_position UNIQUE (journal_id, position)
) STRICT, WITHOUT ROWID;
CREATE INDEX ledger_journalline_account ON ledger_journalline (account_id);

-- Seal + deferred-FK obligation (SAFETY-RULES-PORT section 3.2): the commit-time check pattern. The
-- ledger itself has no deferred rule; layer 2's document journals use these two tables.
CREATE TABLE fb_seal (
  kind   TEXT NOT NULL,
  doc_id TEXT NOT NULL,
  PRIMARY KEY (kind, doc_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE fb_seal_due (
  kind   TEXT NOT NULL,
  doc_id TEXT NOT NULL,
  PRIMARY KEY (kind, doc_id),
  FOREIGN KEY (kind, doc_id) REFERENCES fb_seal (kind, doc_id) DEFERRABLE INITIALLY DEFERRED
) STRICT, WITHOUT ROWID;


INSERT INTO main.fb_user SELECT * FROM temp.copy_fb_user;
DROP TABLE temp.copy_fb_user;
INSERT INTO main.ledger_company SELECT * FROM temp.copy_ledger_company;
DROP TABLE temp.copy_ledger_company;
INSERT INTO main.ledger_company_members SELECT * FROM temp.copy_ledger_company_members;
DROP TABLE temp.copy_ledger_company_members;
INSERT INTO main.ledger_account SELECT * FROM temp.copy_ledger_account;
DROP TABLE temp.copy_ledger_account;
INSERT INTO main.ledger_auditevent SELECT * FROM temp.copy_ledger_auditevent;
DROP TABLE temp.copy_ledger_auditevent;
INSERT INTO main.ledger_journal SELECT * FROM temp.copy_ledger_journal;
DROP TABLE temp.copy_ledger_journal;
INSERT INTO main.ledger_journalline SELECT * FROM temp.copy_ledger_journalline;
DROP TABLE temp.copy_ledger_journalline;
INSERT INTO main.fb_seal SELECT * FROM temp.copy_fb_seal;
DROP TABLE temp.copy_fb_seal;
INSERT INTO main.fb_seal_due SELECT * FROM temp.copy_fb_seal_due;
DROP TABLE temp.copy_fb_seal_due;

-- ---------------------------------------------------------------- guards

CREATE TRIGGER fb_seal_frozen BEFORE UPDATE ON fb_seal
BEGIN SELECT RAISE(ABORT, 'seal_immutable: Seals are permanent'); END;
CREATE TRIGGER fb_seal_kept BEFORE DELETE ON fb_seal
BEGIN SELECT RAISE(ABORT, 'seal_immutable: Seals are permanent'); END;
CREATE TRIGGER fb_seal_due_frozen BEFORE UPDATE ON fb_seal_due
BEGIN SELECT RAISE(ABORT, 'seal_immutable: Seal obligations are permanent'); END;
CREATE TRIGGER fb_seal_due_kept BEFORE DELETE ON fb_seal_due
BEGIN SELECT RAISE(ABORT, 'seal_immutable: Seal obligations are permanent'); END;

-- fb_company_guard (currency half; the document-identity half arrives with layer 2).
CREATE TRIGGER fb_company_guard_insert BEFORE INSERT ON ledger_company
WHEN NEW.currency IS NULL OR length(NEW.currency) <> 3 OR NEW.currency GLOB '*[^A-Z]*'
BEGIN SELECT RAISE(ABORT, 'fb_company_guard: Currency must be three uppercase letters'); END;

CREATE TRIGGER fb_company_guard_update BEFORE UPDATE ON ledger_company
BEGIN
  SELECT RAISE(ABORT, 'fb_company_guard: Update would replace an existing row') WHERE EXISTS (SELECT 1 FROM ledger_company WHERE id IS NOT OLD.id AND id=NEW.id);
  SELECT RAISE(ABORT, 'fb_company_guard: Currency must be three uppercase letters')
  WHERE NEW.currency IS NULL OR length(NEW.currency) <> 3 OR NEW.currency GLOB '*[^A-Z]*';
  SELECT RAISE(ABORT, 'fb_company_guard: Recorded books and commercial documents cannot change currency')
  WHERE NEW.currency <> OLD.currency AND EXISTS (SELECT 1 FROM ledger_journal WHERE company_id = OLD.id);
  SELECT RAISE(ABORT, 'fb_company_guard: Company ids are permanent') WHERE NEW.id IS NOT OLD.id;
END;

-- fb_account_guard.
CREATE TRIGGER fb_account_guard_insert BEFORE INSERT ON ledger_account
WHEN NEW.kind IS NULL OR NEW.kind NOT IN ('ASSET', 'LIABILITY', 'EQUITY', 'INCOME', 'EXPENSE')
BEGIN SELECT RAISE(ABORT, 'fb_account_guard: Invalid account type'); END;

CREATE TRIGGER fb_account_guard_update BEFORE UPDATE ON ledger_account
BEGIN
  SELECT RAISE(ABORT, 'fb_account_guard: Update would replace an existing row') WHERE EXISTS (SELECT 1 FROM ledger_account WHERE id IS NOT OLD.id AND (id=NEW.id OR (company_id=NEW.company_id AND code=NEW.code)));
  SELECT RAISE(ABORT, 'fb_account_guard: Invalid account type')
  WHERE NEW.kind IS NULL OR NEW.kind NOT IN ('ASSET', 'LIABILITY', 'EQUITY', 'INCOME', 'EXPENSE');
  SELECT RAISE(ABORT, 'fb_account_guard: Used accounts cannot change company or type')
  WHERE (NEW.company_id IS NOT OLD.company_id OR NEW.kind IS NOT OLD.kind OR NEW.id IS NOT OLD.id)
    AND EXISTS (SELECT 1 FROM ledger_journalline WHERE account_id = OLD.id);
END;

-- PG fills a missing type inside fb_account_guard (NEW.detail_type := ...). SQLite triggers cannot assign
-- NEW (probe F9), so an AFTER trigger writes the plain type of the kind into the row it just stored.
CREATE TRIGGER fb_account_fill_type_insert AFTER INSERT ON ledger_account WHEN NEW.detail_type = ''
BEGIN
  UPDATE ledger_account SET detail_type = CASE NEW.kind WHEN 'ASSET' THEN 'OTHER_CURRENT_ASSET'
    WHEN 'LIABILITY' THEN 'OTHER_CURRENT_LIABILITY' ELSE NEW.kind END WHERE id = NEW.id;
END;
CREATE TRIGGER fb_account_fill_type_update AFTER UPDATE OF detail_type, kind ON ledger_account WHEN NEW.detail_type = ''
BEGIN
  UPDATE ledger_account SET detail_type = CASE NEW.kind WHEN 'ASSET' THEN 'OTHER_CURRENT_ASSET'
    WHEN 'LIABILITY' THEN 'OTHER_CURRENT_LIABILITY' ELSE NEW.kind END WHERE id = NEW.id;
END;

-- fb_audit_guard. Needs PRAGMA recursive_triggers = ON, or INSERT OR REPLACE rewrites a row (probe B7d).
CREATE TRIGGER fb_audit_no_update BEFORE UPDATE ON ledger_auditevent
BEGIN SELECT RAISE(ABORT, 'fb_audit_guard: Audit events are append-only'); END;
CREATE TRIGGER fb_audit_no_delete BEFORE DELETE ON ledger_auditevent
BEGIN SELECT RAISE(ABORT, 'fb_audit_guard: Audit events are append-only'); END;

-- fb_journal_guard, INSERT half. Checks run in the PostgreSQL function's order.
CREATE TRIGGER fb_journal_guard_insert BEFORE INSERT ON ledger_journal
BEGIN
  SELECT RAISE(ABORT, 'fb_journal_guard: Journals must start as drafts') WHERE NEW.status IS NOT 'DRAFT';
  SELECT RAISE(ABORT, 'journal_retry_evidence: Invalid journal revision or retry evidence')
  WHERE NEW.revision < 1 OR (NEW.request_key IS NOT NULL AND length(NEW.creation_digest) <> 64);
  SELECT RAISE(ABORT, 'fb_journal_guard: Invalid journal state')
  WHERE NEW.posted_at IS NOT NULL OR NEW.posted_by_id IS NOT NULL;
  SELECT RAISE(ABORT, 'journal_entry_kind: Only manual original journal entries carry an entry number or retry key; document journals are never adjusting')
  WHERE NEW.reversal_of_id IS NOT NULL AND (NEW.entry_number <> '' OR NEW.request_key IS NOT NULL);
END;

-- fb_journal_guard, UPDATE half: immutability, ownership, retry evidence, state, and the posting checks
-- at DRAFT -> POSTED (balanced nonzero lines, an operator, eligible accounts, manual bounds, reversals).
CREATE TRIGGER fb_journal_guard_update BEFORE UPDATE ON ledger_journal
BEGIN
  SELECT RAISE(ABORT, 'fb_journal_guard: Update would replace an existing row') WHERE EXISTS (SELECT 1 FROM ledger_journal WHERE id IS NOT OLD.id AND (
    id=NEW.id OR number=NEW.number OR (company_id=NEW.company_id AND entry_number=NEW.entry_number AND NEW.entry_number<>'')
    OR (company_id=NEW.company_id AND request_key=NEW.request_key) OR reversal_of_id=NEW.reversal_of_id));
  SELECT RAISE(ABORT, 'fb_journal_guard: Posted journals are immutable; create a reversal') WHERE OLD.status = 'POSTED';
  SELECT RAISE(ABORT, 'fb_journal_guard: Journal ownership cannot change')
  WHERE NEW.company_id IS NOT OLD.company_id OR NEW.created_by_id IS NOT OLD.created_by_id
     OR NEW.created_at IS NOT OLD.created_at OR NEW.id IS NOT OLD.id OR NEW.number IS NOT OLD.number;
  SELECT RAISE(ABORT, 'journal_retry_evidence: Journal retry evidence and revision cannot change or go back')
  WHERE NEW.request_key IS NOT OLD.request_key OR NEW.creation_digest IS NOT OLD.creation_digest OR NEW.revision < OLD.revision;
  SELECT RAISE(ABORT, 'journal_retry_evidence: Invalid journal revision or retry evidence')
  WHERE NEW.revision < 1 OR (NEW.request_key IS NOT NULL AND length(NEW.creation_digest) <> 64);
  SELECT RAISE(ABORT, 'fb_journal_guard: Invalid journal state')
  WHERE NEW.status IS NULL OR NEW.status NOT IN ('DRAFT', 'POSTED')
     OR (NEW.status = 'DRAFT' AND (NEW.posted_at IS NOT NULL OR NEW.posted_by_id IS NOT NULL));
  SELECT RAISE(ABORT, 'fb_journal_guard: Posting requires balanced nonzero lines and an identified operator')
  WHERE NEW.status = 'POSTED' AND (
        (SELECT count(*) FROM ledger_journalline WHERE journal_id = NEW.id) < 2
     OR coalesce((SELECT sum(debit) FROM ledger_journalline WHERE journal_id = NEW.id), 0) <= 0
     OR coalesce((SELECT sum(debit) FROM ledger_journalline WHERE journal_id = NEW.id), 0)
        <> coalesce((SELECT sum(credit) FROM ledger_journalline WHERE journal_id = NEW.id), 0)
     OR NEW.posted_by_id IS NULL OR NEW.posted_at IS NULL);
  SELECT RAISE(ABORT, 'fb_journal_guard: Invalid posting account')
  WHERE NEW.status = 'POSTED' AND EXISTS (
        SELECT 1 FROM ledger_journalline l JOIN ledger_account a ON a.id = l.account_id
        WHERE l.journal_id = NEW.id AND (a.company_id <> NEW.company_id OR a.active = 0 OR a.postable = 0));
  SELECT RAISE(ABORT, 'journal_entry_bounds: A manual journal entry needs an entry number, at most 50 lines and totals within 99999999999999.99')
  WHERE NEW.status = 'POSTED' AND NEW.reversal_of_id IS NULL AND (
        NEW.entry_number = ''
     OR (SELECT count(*) FROM ledger_journalline WHERE journal_id = NEW.id) > 50
     OR coalesce((SELECT sum(debit) FROM ledger_journalline WHERE journal_id = NEW.id), 0) > 9999999999999999);
  SELECT RAISE(ABORT, 'fb_journal_guard: Invalid reversal')
  WHERE NEW.status = 'POSTED' AND NEW.reversal_of_id IS NOT NULL AND NOT EXISTS (
        SELECT 1 FROM ledger_journal o WHERE o.id = NEW.reversal_of_id AND o.status = 'POSTED'
          AND o.company_id = NEW.company_id AND o.reversal_of_id IS NULL AND NEW.date >= o.date AND NEW.adjusting = o.adjusting);
  -- (journal_id, position) is unique, so EXCEPT both ways is exact (SQLite has no EXCEPT ALL; probe F4).
  SELECT RAISE(ABORT, 'fb_journal_guard: Reversal must exactly compensate original lines')
  WHERE NEW.status = 'POSTED' AND NEW.reversal_of_id IS NOT NULL AND (
        EXISTS (SELECT position, account_id, debit, credit FROM ledger_journalline WHERE journal_id = NEW.reversal_of_id
                EXCEPT SELECT position, account_id, credit, debit FROM ledger_journalline WHERE journal_id = NEW.id)
     OR EXISTS (SELECT position, account_id, credit, debit FROM ledger_journalline WHERE journal_id = NEW.id
                EXCEPT SELECT position, account_id, debit, credit FROM ledger_journalline WHERE journal_id = NEW.reversal_of_id));
  SELECT RAISE(ABORT, 'journal_entry_kind: Only manual original journal entries carry an entry number or retry key; document journals are never adjusting')
  WHERE NEW.reversal_of_id IS NOT NULL AND (NEW.entry_number <> '' OR NEW.request_key IS NOT NULL);
END;

CREATE TRIGGER fb_journal_guard_delete BEFORE DELETE ON ledger_journal WHEN OLD.status = 'POSTED'
BEGIN SELECT RAISE(ABORT, 'fb_journal_guard: Posted journals are immutable; create a reversal'); END;

-- fb_line_guard. (PG's NaN/Infinity test is dropped: STRICT INTEGER columns cannot hold them; probe B11a.)
CREATE TRIGGER fb_line_guard_insert BEFORE INSERT ON ledger_journalline
BEGIN
  SELECT RAISE(ABORT, 'fb_line_guard: Cannot add or move lines to posted journals')
  WHERE (SELECT status FROM ledger_journal WHERE id = NEW.journal_id) = 'POSTED';
  SELECT RAISE(ABORT, 'fb_line_guard: Line must use an active posting account in its company')
  WHERE EXISTS (SELECT 1 FROM ledger_account a, ledger_journal j WHERE a.id = NEW.account_id AND j.id = NEW.journal_id
                AND (a.company_id <> j.company_id OR a.active = 0 OR a.postable = 0));
END;

CREATE TRIGGER fb_line_guard_update BEFORE UPDATE ON ledger_journalline
BEGIN
  SELECT RAISE(ABORT, 'fb_line_guard: Update would replace an existing row') WHERE EXISTS (SELECT 1 FROM ledger_journalline WHERE id IS NOT OLD.id AND (id=NEW.id OR (journal_id=NEW.journal_id AND position=NEW.position)));
  SELECT RAISE(ABORT, 'fb_line_guard: Posted lines are immutable')
  WHERE (SELECT status FROM ledger_journal WHERE id = OLD.journal_id) = 'POSTED';
  SELECT RAISE(ABORT, 'fb_line_guard: Cannot add or move lines to posted journals')
  WHERE (SELECT status FROM ledger_journal WHERE id = NEW.journal_id) = 'POSTED';
  SELECT RAISE(ABORT, 'fb_line_guard: Line must use an active posting account in its company')
  WHERE EXISTS (SELECT 1 FROM ledger_account a, ledger_journal j WHERE a.id = NEW.account_id AND j.id = NEW.journal_id
                AND (a.company_id <> j.company_id OR a.active = 0 OR a.postable = 0));
END;

CREATE TRIGGER fb_line_guard_delete BEFORE DELETE ON ledger_journalline
WHEN (SELECT status FROM ledger_journal WHERE id = OLD.journal_id) = 'POSTED'
BEGIN SELECT RAISE(ABORT, 'fb_line_guard: Posted lines are immutable'); END;

-- L1 review hardening: replacement collisions, parent existence, revision bounds.

DROP TRIGGER fb_company_guard_insert;
CREATE TRIGGER fb_company_guard_insert BEFORE INSERT ON ledger_company
BEGIN
  SELECT RAISE(ABORT, 'fb_company_guard: Company key already exists') WHERE EXISTS (SELECT 1 FROM ledger_company WHERE id=NEW.id);
  SELECT RAISE(ABORT, 'fb_company_guard: Currency must be three uppercase letters') WHERE NEW.currency IS NULL OR length(NEW.currency) <> 3 OR NEW.currency GLOB '*[^A-Z]*';
END;

DROP TRIGGER fb_account_guard_insert;
CREATE TRIGGER fb_account_guard_insert BEFORE INSERT ON ledger_account
BEGIN
  SELECT RAISE(ABORT, 'fb_account_guard: Account key already exists') WHERE EXISTS (SELECT 1 FROM ledger_account WHERE id=NEW.id OR (company_id=NEW.company_id AND code=NEW.code));
  SELECT RAISE(ABORT, 'fb_account_guard: Invalid account type') WHERE NEW.kind IS NULL OR NEW.kind NOT IN ('ASSET', 'LIABILITY', 'EQUITY', 'INCOME', 'EXPENSE');
END;

DROP TRIGGER fb_journal_guard_insert;
CREATE TRIGGER fb_journal_guard_insert BEFORE INSERT ON ledger_journal
BEGIN
  SELECT RAISE(ABORT, 'fb_journal_guard: Journal key already exists') WHERE EXISTS (
    SELECT 1 FROM ledger_journal WHERE id=NEW.id OR number=NEW.number
      OR (company_id=NEW.company_id AND entry_number=NEW.entry_number AND NEW.entry_number<>'')
      OR (company_id=NEW.company_id AND request_key=NEW.request_key)
      OR reversal_of_id=NEW.reversal_of_id);
  SELECT RAISE(ABORT, 'fb_journal_guard: Journals must start as drafts') WHERE NEW.status IS NOT 'DRAFT';
  SELECT RAISE(ABORT, 'journal_retry_evidence: Invalid journal revision or retry evidence')
  WHERE NEW.revision NOT BETWEEN 1 AND 9223372036854775806 OR (NEW.request_key IS NOT NULL AND length(NEW.creation_digest) <> 64);
  SELECT RAISE(ABORT, 'fb_journal_guard: Invalid journal state')
  WHERE NEW.posted_at IS NOT NULL OR NEW.posted_by_id IS NOT NULL;
  SELECT RAISE(ABORT, 'journal_entry_kind: Only manual original journal entries carry an entry number or retry key; document journals are never adjusting')
  WHERE NEW.reversal_of_id IS NOT NULL AND (NEW.entry_number <> '' OR NEW.request_key IS NOT NULL);
END;

DROP TRIGGER fb_journal_guard_update;
CREATE TRIGGER fb_journal_guard_update BEFORE UPDATE ON ledger_journal
BEGIN
  SELECT RAISE(ABORT, 'fb_journal_guard: Update would replace an existing row') WHERE EXISTS (SELECT 1 FROM ledger_journal WHERE id IS NOT OLD.id AND (
    id=NEW.id OR number=NEW.number OR (company_id=NEW.company_id AND entry_number=NEW.entry_number AND NEW.entry_number<>'')
    OR (company_id=NEW.company_id AND request_key=NEW.request_key) OR reversal_of_id=NEW.reversal_of_id));
  SELECT RAISE(ABORT, 'fb_journal_guard: Posted journals are immutable; create a reversal') WHERE OLD.status = 'POSTED';
  SELECT RAISE(ABORT, 'fb_journal_guard: Journal ownership cannot change')
  WHERE NEW.company_id IS NOT OLD.company_id OR NEW.created_by_id IS NOT OLD.created_by_id
     OR NEW.created_at IS NOT OLD.created_at OR NEW.id IS NOT OLD.id OR NEW.number IS NOT OLD.number;
  SELECT RAISE(ABORT, 'journal_retry_evidence: Journal retry evidence and revision cannot change or go back')
  WHERE NEW.request_key IS NOT OLD.request_key OR NEW.creation_digest IS NOT OLD.creation_digest OR NEW.revision < OLD.revision;
  SELECT RAISE(ABORT, 'journal_retry_evidence: Invalid journal revision or retry evidence')
  WHERE NEW.revision NOT BETWEEN 1 AND 9223372036854775806 OR (NEW.request_key IS NOT NULL AND length(NEW.creation_digest) <> 64);
  SELECT RAISE(ABORT, 'fb_journal_guard: Invalid journal state')
  WHERE NEW.status IS NULL OR NEW.status NOT IN ('DRAFT', 'POSTED')
     OR (NEW.status = 'DRAFT' AND (NEW.posted_at IS NOT NULL OR NEW.posted_by_id IS NOT NULL));
  SELECT RAISE(ABORT, 'fb_journal_guard: Posting requires balanced nonzero lines and an identified operator')
  WHERE NEW.status = 'POSTED' AND (
        (SELECT count(*) FROM ledger_journalline WHERE journal_id = NEW.id) < 2
     OR coalesce((SELECT sum(debit) FROM ledger_journalline WHERE journal_id = NEW.id), 0) <= 0
     OR coalesce((SELECT sum(debit) FROM ledger_journalline WHERE journal_id = NEW.id), 0)
        <> coalesce((SELECT sum(credit) FROM ledger_journalline WHERE journal_id = NEW.id), 0)
     OR NEW.posted_by_id IS NULL OR NEW.posted_at IS NULL);
  SELECT RAISE(ABORT, 'fb_journal_guard: Invalid posting account')
  WHERE NEW.status = 'POSTED' AND EXISTS (
        SELECT 1 FROM ledger_journalline l LEFT JOIN ledger_account a ON a.id = l.account_id
        WHERE l.journal_id = NEW.id AND (a.id IS NULL OR a.company_id <> NEW.company_id OR a.active = 0 OR a.postable = 0));
  SELECT RAISE(ABORT, 'journal_entry_bounds: A manual journal entry needs an entry number, at most 50 lines and totals within 99999999999999.99')
  WHERE NEW.status = 'POSTED' AND NEW.reversal_of_id IS NULL AND (
        NEW.entry_number = ''
     OR (SELECT count(*) FROM ledger_journalline WHERE journal_id = NEW.id) > 50
     OR coalesce((SELECT sum(debit) FROM ledger_journalline WHERE journal_id = NEW.id), 0) > 9999999999999999);
  SELECT RAISE(ABORT, 'fb_journal_guard: Invalid reversal')
  WHERE NEW.status = 'POSTED' AND NEW.reversal_of_id IS NOT NULL AND NOT EXISTS (
        SELECT 1 FROM ledger_journal o WHERE o.id = NEW.reversal_of_id AND o.status = 'POSTED'
          AND o.company_id = NEW.company_id AND o.reversal_of_id IS NULL AND NEW.date >= o.date AND NEW.adjusting = o.adjusting);
  -- (journal_id, position) is unique, so EXCEPT both ways is exact (SQLite has no EXCEPT ALL; probe F4).
  SELECT RAISE(ABORT, 'fb_journal_guard: Reversal must exactly compensate original lines')
  WHERE NEW.status = 'POSTED' AND NEW.reversal_of_id IS NOT NULL AND (
        EXISTS (SELECT position, account_id, debit, credit FROM ledger_journalline WHERE journal_id = NEW.reversal_of_id
                EXCEPT SELECT position, account_id, credit, debit FROM ledger_journalline WHERE journal_id = NEW.id)
     OR EXISTS (SELECT position, account_id, credit, debit FROM ledger_journalline WHERE journal_id = NEW.id
                EXCEPT SELECT position, account_id, debit, credit FROM ledger_journalline WHERE journal_id = NEW.reversal_of_id));
  SELECT RAISE(ABORT, 'journal_entry_kind: Only manual original journal entries carry an entry number or retry key; document journals are never adjusting')
  WHERE NEW.reversal_of_id IS NOT NULL AND (NEW.entry_number <> '' OR NEW.request_key IS NOT NULL);
END;

DROP TRIGGER fb_journal_guard_delete;
CREATE TRIGGER fb_journal_guard_delete BEFORE DELETE ON ledger_journal WHEN OLD.status = 'POSTED' OR EXISTS (SELECT 1 FROM ledger_journalline WHERE journal_id=OLD.id) OR EXISTS (SELECT 1 FROM ledger_journal WHERE reversal_of_id=OLD.id)
BEGIN SELECT RAISE(ABORT, 'fb_journal_guard: Posted journals are immutable; create a reversal'); END;

DROP TRIGGER fb_line_guard_insert;
CREATE TRIGGER fb_line_guard_insert BEFORE INSERT ON ledger_journalline
BEGIN
  SELECT RAISE(ABORT, 'fb_line_guard: Line key already exists') WHERE EXISTS (SELECT 1 FROM ledger_journalline WHERE id=NEW.id OR (journal_id=NEW.journal_id AND position=NEW.position));
  SELECT RAISE(ABORT, 'fb_line_guard: Cannot add or move lines to posted journals')
  WHERE (SELECT status FROM ledger_journal WHERE id = NEW.journal_id) IS NOT 'DRAFT';
  SELECT RAISE(ABORT, 'fb_line_guard: Line must use an active posting account in its company')
  WHERE NOT EXISTS (SELECT 1 FROM ledger_account a, ledger_journal j WHERE a.id = NEW.account_id AND j.id = NEW.journal_id
                AND a.company_id = j.company_id AND a.active = 1 AND a.postable = 1);
END;

DROP TRIGGER fb_line_guard_update;
CREATE TRIGGER fb_line_guard_update BEFORE UPDATE ON ledger_journalline
BEGIN
  SELECT RAISE(ABORT, 'fb_line_guard: Update would replace an existing row') WHERE EXISTS (SELECT 1 FROM ledger_journalline WHERE id IS NOT OLD.id AND (id=NEW.id OR (journal_id=NEW.journal_id AND position=NEW.position)));
  SELECT RAISE(ABORT, 'fb_line_guard: Posted lines are immutable')
  WHERE (SELECT status FROM ledger_journal WHERE id = OLD.journal_id) = 'POSTED';
  SELECT RAISE(ABORT, 'fb_line_guard: Cannot add or move lines to posted journals')
  WHERE (SELECT status FROM ledger_journal WHERE id = NEW.journal_id) IS NOT 'DRAFT';
  SELECT RAISE(ABORT, 'fb_line_guard: Line must use an active posting account in its company')
  WHERE NOT EXISTS (SELECT 1 FROM ledger_account a, ledger_journal j WHERE a.id = NEW.account_id AND j.id = NEW.journal_id
                AND a.company_id = j.company_id AND a.active = 1 AND a.postable = 1);
END;


CREATE TRIGGER fb_company_guard_delete BEFORE DELETE ON ledger_company
WHEN EXISTS (SELECT 1 FROM ledger_journal WHERE company_id=OLD.id)
  OR EXISTS (SELECT 1 FROM ledger_account WHERE company_id=OLD.id)
  OR EXISTS (SELECT 1 FROM ledger_auditevent WHERE company_id=OLD.id)
  OR EXISTS (SELECT 1 FROM ledger_company_members WHERE company_id=OLD.id)
BEGIN SELECT RAISE(ABORT, 'fb_company_guard: Referenced companies cannot be deleted'); END;
CREATE TRIGGER fb_account_guard_delete BEFORE DELETE ON ledger_account
WHEN EXISTS (SELECT 1 FROM ledger_journalline WHERE account_id=OLD.id)
BEGIN SELECT RAISE(ABORT, 'fb_account_guard: Used accounts cannot be deleted'); END;
CREATE TRIGGER fb_audit_guard_insert BEFORE INSERT ON ledger_auditevent
WHEN EXISTS (SELECT 1 FROM ledger_auditevent WHERE id=NEW.id)
BEGIN SELECT RAISE(ABORT, 'fb_audit_guard: Audit events are append-only'); END;
CREATE TRIGGER fb_user_guard_insert BEFORE INSERT ON fb_user
WHEN EXISTS (SELECT 1 FROM fb_user WHERE id=NEW.id OR username=NEW.username)
BEGIN SELECT RAISE(ABORT, 'fb_user_guard: Operator key already exists'); END;
CREATE TRIGGER fb_user_guard_delete BEFORE DELETE ON fb_user
WHEN EXISTS (SELECT 1 FROM ledger_auditevent WHERE actor_id=OLD.id)
  OR EXISTS (SELECT 1 FROM ledger_journal WHERE created_by_id=OLD.id OR posted_by_id=OLD.id)
  OR EXISTS (SELECT 1 FROM ledger_company_members WHERE user_id=OLD.id)
BEGIN SELECT RAISE(ABORT, 'fb_user_guard: Referenced operators cannot be deleted'); END;
CREATE TRIGGER fb_seal_guard_insert BEFORE INSERT ON fb_seal
WHEN EXISTS (SELECT 1 FROM fb_seal WHERE kind=NEW.kind AND doc_id=NEW.doc_id)
BEGIN SELECT RAISE(ABORT, 'seal_immutable: Seals are permanent'); END;
CREATE TRIGGER fb_seal_due_guard_insert BEFORE INSERT ON fb_seal_due
WHEN EXISTS (SELECT 1 FROM fb_seal_due WHERE kind=NEW.kind AND doc_id=NEW.doc_id)
BEGIN SELECT RAISE(ABORT, 'seal_immutable: Seal obligations are permanent'); END;

CREATE TRIGGER fb_user_guard_update BEFORE UPDATE ON fb_user
BEGIN
  SELECT RAISE(ABORT, 'fb_user_guard: Update would replace an existing operator')
  WHERE EXISTS (SELECT 1 FROM fb_user WHERE id IS NOT OLD.id AND (id=NEW.id OR username=NEW.username));
END;

CREATE TRIGGER fb_members_guard_update BEFORE UPDATE ON ledger_company_members
BEGIN
  SELECT RAISE(ABORT, 'fb_members_guard: Update would replace an existing membership')
  WHERE EXISTS (SELECT 1 FROM ledger_company_members
    WHERE (company_id IS NOT OLD.company_id OR user_id IS NOT OLD.user_id)
      AND company_id=NEW.company_id AND user_id=NEW.user_id);
END;
