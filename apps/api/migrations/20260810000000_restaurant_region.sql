ALTER TABLE restaurants
    ADD COLUMN region TEXT
        CHECK (region IS NULL OR (BTRIM(region) <> '' AND CHAR_LENGTH(region) <= 100));
