-- The stored picker is an eligibility boundary, including raw Writer DML.
CREATE VIEW assistant_selection_invalid AS
 SELECT id FROM assistant_connection WHERE NOT fb_assistant_model(model,model_choices);
CREATE TRIGGER fb_assistant_model_insert BEFORE INSERT ON assistant_connection BEGIN
 SELECT RAISE(ABORT,'assistant_model: Choose a model from the stored list') WHERE NOT fb_assistant_model(NEW.model,NEW.model_choices);
END;
CREATE TRIGGER fb_assistant_model_update BEFORE UPDATE ON assistant_connection BEGIN
 SELECT RAISE(ABORT,'assistant_model: Choose a model from the stored list') WHERE NOT fb_assistant_model(NEW.model,NEW.model_choices);
END;
