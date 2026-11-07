-- First-run only. Never adopt even an otherwise-unused legacy operator.
DROP TRIGGER fb_owner_insert;
CREATE TRIGGER fb_owner_insert BEFORE INSERT ON accounts_owner BEGIN
    SELECT RAISE(ABORT, 'owner_guard: Owner already exists')
      WHERE EXISTS(SELECT 1 FROM accounts_owner);
    SELECT RAISE(ABORT, 'owner_guard: One active staff owner is required in an empty-book command')
      WHERE NOT fb_owner_empty_command()
         OR (SELECT count(*) FROM fb_user) != 1
         OR NOT EXISTS(SELECT 1 FROM fb_user WHERE id=NEW.user_id AND is_active=1 AND is_staff=1)
         OR EXISTS(SELECT 1 FROM ledger_auditevent WHERE actor_id=NEW.user_id)
         OR EXISTS(SELECT 1 FROM ledger_company_members WHERE user_id=NEW.user_id);
END;
