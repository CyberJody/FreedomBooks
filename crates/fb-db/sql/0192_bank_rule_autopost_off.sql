-- Preserve imported conditions and provenance. Only turning Auto-post off is editable.
-- No existing rule is changed by this migration.
DROP TRIGGER fb_rule_policy_fixed;
CREATE TRIGGER fb_rule_policy_fixed BEFORE UPDATE ON bankfeeds_rule_policy BEGIN
 SELECT RAISE(ABORT,'rule_policy_fixed: Only turning off Auto-post is allowed; imported conditions and history are retained')
 WHERE NEW.rule_id IS NOT OLD.rule_id OR NEW.company_id IS NOT OLD.company_id
 OR NEW.source_key IS NOT OLD.source_key OR NEW.source_hash IS NOT OLD.source_hash
 OR NEW.source_row IS NOT OLD.source_row
 OR json_type(OLD.data,'$.auto_post') IS NOT 'true'
 OR json_type(NEW.data,'$.auto_post') IS NOT 'false'
 OR json_remove(NEW.data,'$.auto_post') IS NOT json_remove(OLD.data,'$.auto_post');
END;
