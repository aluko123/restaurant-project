ALTER TABLE restaurants
    ADD COLUMN country TEXT
        CHECK (country IS NULL OR (BTRIM(country) <> '' AND CHAR_LENGTH(country) <= 100));
