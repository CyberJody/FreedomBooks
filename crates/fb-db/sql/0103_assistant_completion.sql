-- Finished outcomes must complete a pending attempt, never an expired one.
CREATE TRIGGER fb_assistant_reading_complete_pending BEFORE UPDATE ON assistant_reading
WHEN (NEW.status='ok' OR (NEW.status='failed' AND NEW.finished_at IS NOT NULL))
 AND (NEW.status IS NOT OLD.status OR NEW.finished_at IS NOT OLD.finished_at)
 AND OLD.status<>'pending'
BEGIN
 SELECT RAISE(ABORT,'assistant_reading: Claim a pending attempt before completing');
END;
