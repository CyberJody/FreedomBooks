-- Desktop owner is opt-in for legacy synthetic/multi-user books. Once selected,
-- the actor identity cannot be replaced or deactivated. Existing history is kept.
CREATE TABLE accounts_owner (
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    user_id TEXT NOT NULL UNIQUE REFERENCES fb_user(id)
      CHECK(length(user_id)=36 AND substr(user_id,9,1)='-' AND substr(user_id,14,1)='-' AND substr(user_id,19,1)='-' AND substr(user_id,24,1)='-'
        AND length(replace(user_id,'-',''))=32 AND replace(user_id,'-','') NOT GLOB '*[^0-9a-f]*')
) STRICT, WITHOUT ROWID;
CREATE TRIGGER fb_owner_insert BEFORE INSERT ON accounts_owner BEGIN
    SELECT RAISE(ABORT, 'owner_guard: Owner already exists')
      WHERE EXISTS(SELECT 1 FROM accounts_owner);
    SELECT RAISE(ABORT, 'owner_guard: One active staff owner is required')
      WHERE (SELECT count(*) FROM fb_user) != 1 OR NOT EXISTS(
        SELECT 1 FROM fb_user WHERE id=NEW.user_id AND is_active=1 AND is_staff=1);
END;
CREATE TRIGGER fb_owner_update BEFORE UPDATE ON accounts_owner BEGIN
    SELECT RAISE(ABORT, 'owner_guard: Owner identity is permanent');
END;
CREATE TRIGGER fb_owner_delete BEFORE DELETE ON accounts_owner BEGIN
    SELECT RAISE(ABORT, 'owner_guard: Owner cannot be deleted');
END;
CREATE TRIGGER fb_owner_user_insert BEFORE INSERT ON fb_user
WHEN EXISTS(SELECT 1 FROM accounts_owner) BEGIN
    SELECT RAISE(ABORT, 'owner_guard: This book has one owner');
END;
CREATE TRIGGER fb_owner_user_update BEFORE UPDATE ON fb_user
WHEN EXISTS(SELECT 1 FROM accounts_owner WHERE user_id=OLD.id)
 AND (NEW.id IS NOT OLD.id OR NEW.is_active != 1 OR NEW.is_staff != 1) BEGIN
    SELECT RAISE(ABORT, 'owner_guard: Owner must remain active staff');
END;
CREATE TRIGGER fb_owner_user_delete BEFORE DELETE ON fb_user
WHEN EXISTS(SELECT 1 FROM accounts_owner WHERE user_id=OLD.id) BEGIN
    SELECT RAISE(ABORT, 'owner_guard: Owner cannot be deleted');
END;
