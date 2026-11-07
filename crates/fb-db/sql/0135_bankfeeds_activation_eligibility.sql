-- Bank feeds FIX2 (review 2 of 0134): reactivating a mapped feed re-runs the whole mapping predicate of
-- 0133's fb_bankfeeds_account_update (services.map_account), not only the duplicate check: the mapped
-- account must still be an active postable ASSET/LIABILITY, non-control account of the company
-- (bankfeeds_mappable), the feed's currency the company's, and no other active feed of the company may
-- hold that account. Neither Python nor the v2 services ever reactivate a feed, so this refuses only
-- direct writes; open-time checks are unchanged, so every service-reachable account edit stays openable.
DROP TRIGGER fb_bankfeeds_account_activation;
CREATE TRIGGER fb_bankfeeds_account_activation BEFORE UPDATE ON bankfeeds_account WHEN NEW.active=1 AND OLD.active IS NOT 1 AND NEW.ledger_account_id IS NOT NULL BEGIN
 SELECT RAISE(ABORT,'bankfeeds_mapping: Map a feed to an eligible unmapped bank or credit card account of its company') WHERE
   NOT EXISTS(SELECT 1 FROM bankfeeds_mappable WHERE account_id=NEW.ledger_account_id AND connection_id=NEW.connection_id)
   OR NOT EXISTS(SELECT 1 FROM bankfeeds_connection c JOIN ledger_company y ON y.id=c.company_id WHERE c.id=NEW.connection_id AND upper(NEW.currency)=y.currency)
   OR EXISTS(SELECT 1 FROM bankfeeds_account f JOIN bankfeeds_connection fc ON fc.id=f.connection_id JOIN bankfeeds_connection nc ON nc.company_id=fc.company_id
     WHERE nc.id=NEW.connection_id AND f.id IS NOT NEW.id AND f.ledger_account_id=NEW.ledger_account_id AND f.active=1);
END;
