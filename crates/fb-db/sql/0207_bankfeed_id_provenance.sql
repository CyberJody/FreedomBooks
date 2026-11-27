-- Old provider IDs may have been normalized before the core received them.
-- Only newly staged IDs that survived the provider parser can be compared across apps.
ALTER TABLE bankfeeds_account ADD COLUMN provider_id_verified INTEGER NOT NULL DEFAULT 0
 CHECK(provider_id_verified IN (0,1));
ALTER TABLE bankfeeds_transaction ADD COLUMN provider_id_verified INTEGER NOT NULL DEFAULT 0
 CHECK(provider_id_verified IN (0,1));
