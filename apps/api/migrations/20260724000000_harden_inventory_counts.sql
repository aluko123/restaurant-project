ALTER TABLE inventory_count_sessions
    ADD COLUMN revision BIGINT NOT NULL DEFAULT 0 CHECK (revision >= 0);

CREATE OR REPLACE FUNCTION protect_inventory_completed_entry() RETURNS TRIGGER AS $$
DECLARE
    target_session UUID;
    session_status TEXT;
BEGIN
    target_session := CASE WHEN TG_OP = 'DELETE' THEN OLD.session_id ELSE NEW.session_id END;

    SELECT status INTO session_status
    FROM inventory_count_sessions
    WHERE id = target_session
    FOR UPDATE;

    IF session_status = 'completed' THEN
        RAISE EXCEPTION 'completed inventory count entries are immutable';
    END IF;
    IF TG_OP = 'UPDATE' AND NEW.session_id <> OLD.session_id THEN
        RAISE EXCEPTION 'inventory count entries cannot move between sessions';
    END IF;
    RETURN CASE WHEN TG_OP = 'DELETE' THEN OLD ELSE NEW END;
END;
$$ LANGUAGE plpgsql;
