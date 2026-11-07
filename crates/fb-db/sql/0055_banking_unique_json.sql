-- Reject ambiguous JSON before SQL and Rust can interpret it differently.
CREATE VIEW banking_json_invalid AS
SELECT id FROM banking_deposit WHERE fb_json_unique_keys(data)<>1
UNION ALL SELECT id FROM banking_depositreversal WHERE fb_json_unique_keys(data)<>1
UNION ALL SELECT id FROM banking_reconciliation WHERE fb_json_unique_keys(data)<>1;
CREATE TRIGGER fb_banking_deposit_json_insert BEFORE INSERT ON banking_deposit BEGIN SELECT RAISE(ABORT,'banking_evidence: Invalid or duplicate JSON keys') WHERE fb_json_unique_keys(NEW.data)<>1; END;
CREATE TRIGGER fb_banking_deposit_json_update BEFORE UPDATE ON banking_deposit BEGIN SELECT RAISE(ABORT,'banking_evidence: Invalid or duplicate JSON keys') WHERE fb_json_unique_keys(NEW.data)<>1; END;
CREATE TRIGGER fb_banking_depositreversal_json_insert BEFORE INSERT ON banking_depositreversal BEGIN SELECT RAISE(ABORT,'banking_evidence: Invalid or duplicate JSON keys') WHERE fb_json_unique_keys(NEW.data)<>1; END;
CREATE TRIGGER fb_banking_depositreversal_json_update BEFORE UPDATE ON banking_depositreversal BEGIN SELECT RAISE(ABORT,'banking_evidence: Invalid or duplicate JSON keys') WHERE fb_json_unique_keys(NEW.data)<>1; END;
CREATE TRIGGER fb_banking_reconciliation_json_insert BEFORE INSERT ON banking_reconciliation BEGIN SELECT RAISE(ABORT,'banking_evidence: Invalid or duplicate JSON keys') WHERE fb_json_unique_keys(NEW.data)<>1; END;
CREATE TRIGGER fb_banking_reconciliation_json_update BEFORE UPDATE ON banking_reconciliation BEGIN SELECT RAISE(ABORT,'banking_evidence: Invalid or duplicate JSON keys') WHERE fb_json_unique_keys(NEW.data)<>1; END;
