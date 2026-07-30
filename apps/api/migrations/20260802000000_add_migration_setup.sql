ALTER TABLE restaurants
    ADD COLUMN pos_system TEXT CHECK (
        pos_system IS NULL OR (BTRIM(pos_system) <> '' AND CHAR_LENGTH(pos_system) <= 80)
    ),
    ADD COLUMN accounting_system TEXT CHECK (
        accounting_system IS NULL OR (BTRIM(accounting_system) <> '' AND CHAR_LENGTH(accounting_system) <= 80)
    ),
    ADD COLUMN migration_setup_completed_at TIMESTAMPTZ;
