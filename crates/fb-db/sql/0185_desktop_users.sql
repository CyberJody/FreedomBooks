-- Named local operators. Encryption and its recovery protocol are unchanged.
-- Retain the original owner mapping and audit identities, but retire the one-user restriction.
DROP TRIGGER fb_owner_user_insert;
DROP TRIGGER fb_owner_user_update;
CREATE TRIGGER fb_owner_user_insert BEFORE INSERT ON fb_user
WHEN EXISTS(SELECT 1 FROM accounts_owner WHERE user_id=NEW.id)
BEGIN SELECT RAISE(ABORT, 'owner_guard: Owner identity is permanent'); END;
CREATE TRIGGER fb_owner_user_update BEFORE UPDATE ON fb_user
WHEN EXISTS(SELECT 1 FROM accounts_owner WHERE user_id=OLD.id) AND NEW.id IS NOT OLD.id
BEGIN SELECT RAISE(ABORT, 'owner_guard: Owner identity is permanent'); END;

CREATE TABLE desktop_user (
    user_id TEXT PRIMARY KEY REFERENCES fb_user(id),
    role TEXT NOT NULL CHECK(role IN ('administrator','bookkeeper','readonly')),
    password_hash TEXT CHECK(password_hash IS NULL OR (length(password_hash) BETWEEN 80 AND 512 AND password_hash GLOB '$argon2id$v=19$*'))
) STRICT, WITHOUT ROWID;
INSERT INTO desktop_user(user_id,role)
    SELECT id, CASE WHEN is_staff=1 OR is_superuser=1 THEN 'administrator' ELSE 'bookkeeper' END FROM fb_user;

CREATE TRIGGER desktop_user_identity BEFORE UPDATE ON desktop_user WHEN NEW.user_id IS NOT OLD.user_id
BEGIN SELECT RAISE(ABORT, 'desktop_user: Identity is permanent'); END;
CREATE TRIGGER desktop_user_kept BEFORE DELETE ON desktop_user
BEGIN SELECT RAISE(ABORT, 'desktop_user: Disable users instead of deleting them'); END;
CREATE TRIGGER desktop_user_password_kept BEFORE UPDATE ON desktop_user WHEN OLD.password_hash IS NOT NULL AND NEW.password_hash IS NULL
BEGIN SELECT RAISE(ABORT, 'desktop_user: A configured password cannot be removed'); END;
CREATE TRIGGER desktop_user_last_admin BEFORE UPDATE ON desktop_user
WHEN OLD.role='administrator' AND OLD.password_hash IS NOT NULL AND NEW.role!='administrator'
BEGIN
    SELECT RAISE(ABORT, 'desktop_user: Keep an active administrator with a password')
    WHERE NOT EXISTS(SELECT 1 FROM desktop_user d JOIN fb_user u ON u.id=d.user_id
      WHERE d.user_id!=OLD.user_id AND d.role='administrator' AND d.password_hash IS NOT NULL AND u.is_active=1);
    SELECT RAISE(ABORT, 'desktop_user: Keep an administrator for every company')
    WHERE EXISTS(SELECT 1 FROM ledger_company_members m WHERE m.user_id=OLD.user_id AND NOT EXISTS(
      SELECT 1 FROM ledger_company_members n JOIN desktop_user d ON d.user_id=n.user_id JOIN fb_user u ON u.id=d.user_id
      WHERE n.company_id=m.company_id AND n.user_id!=OLD.user_id AND d.role='administrator' AND d.password_hash IS NOT NULL AND u.is_active=1));
END;
CREATE TRIGGER desktop_user_last_active BEFORE UPDATE ON fb_user
WHEN OLD.is_active=1 AND NEW.is_active=0 AND EXISTS(SELECT 1 FROM desktop_user WHERE user_id=OLD.id AND role='administrator' AND password_hash IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'desktop_user: Keep an active administrator with a password')
    WHERE NOT EXISTS(SELECT 1 FROM desktop_user d JOIN fb_user u ON u.id=d.user_id
      WHERE d.user_id!=OLD.id AND d.role='administrator' AND d.password_hash IS NOT NULL AND u.is_active=1);
    SELECT RAISE(ABORT, 'desktop_user: Keep an administrator for every company')
    WHERE EXISTS(SELECT 1 FROM ledger_company_members m WHERE m.user_id=OLD.id AND NOT EXISTS(
      SELECT 1 FROM ledger_company_members n JOIN desktop_user d ON d.user_id=n.user_id JOIN fb_user u ON u.id=d.user_id
      WHERE n.company_id=m.company_id AND n.user_id!=OLD.id AND d.role='administrator' AND d.password_hash IS NOT NULL AND u.is_active=1));
END;
CREATE TRIGGER desktop_user_last_member BEFORE DELETE ON ledger_company_members
WHEN EXISTS(SELECT 1 FROM desktop_user d JOIN fb_user u ON u.id=d.user_id WHERE d.user_id=OLD.user_id AND d.role='administrator' AND d.password_hash IS NOT NULL AND u.is_active=1)
BEGIN
    SELECT RAISE(ABORT, 'desktop_user: Keep an administrator for every company')
    WHERE NOT EXISTS(SELECT 1 FROM ledger_company_members n JOIN desktop_user d ON d.user_id=n.user_id JOIN fb_user u ON u.id=d.user_id
      WHERE n.company_id=OLD.company_id AND n.user_id!=OLD.user_id AND d.role='administrator' AND d.password_hash IS NOT NULL AND u.is_active=1);
END;

-- Open-time verification also detects a damaged enrollment/administration state.
CREATE VIEW desktop_user_invalid AS
SELECT 'no active enrolled administrator' AS problem
WHERE EXISTS(SELECT 1 FROM desktop_user WHERE password_hash IS NOT NULL)
AND NOT EXISTS(SELECT 1 FROM desktop_user d JOIN fb_user u ON u.id=d.user_id
  WHERE d.role='administrator' AND d.password_hash IS NOT NULL AND u.is_active=1)
UNION ALL
SELECT 'company without an enrolled administrator: ' || c.id FROM ledger_company c
WHERE EXISTS(SELECT 1 FROM desktop_user WHERE password_hash IS NOT NULL)
AND NOT EXISTS(SELECT 1 FROM ledger_company_members m JOIN desktop_user d ON d.user_id=m.user_id JOIN fb_user u ON u.id=d.user_id
  WHERE m.company_id=c.id AND d.role='administrator' AND d.password_hash IS NOT NULL AND u.is_active=1);
