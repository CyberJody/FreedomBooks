-- Rust UDF checks the exact HALF_UP product. It is installed by each payables write.
-- A foreign connection without the UDF fails closed; SQL never performs financial multiplication.
CREATE TRIGGER fb_bill_line_calculation BEFORE INSERT ON payables_billline
WHEN NOT fb_payables_line_valid(NEW.quantity,NEW.rate,NEW.amount)
BEGIN SELECT RAISE(ABORT,'bill_line_calculation: Invalid bill line calculation'); END;
