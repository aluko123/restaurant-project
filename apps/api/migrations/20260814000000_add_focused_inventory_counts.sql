ALTER TABLE inventory_count_sessions
    DROP CONSTRAINT inventory_count_sessions_scope_check;

ALTER TABLE inventory_count_sessions
    ADD CONSTRAINT inventory_count_sessions_scope_check
    CHECK (scope IN ('all', 'areas', 'focused'));
