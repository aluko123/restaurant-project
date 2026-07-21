ALTER TABLE menu_items
    DROP CONSTRAINT menu_items_name_check,
    DROP CONSTRAINT menu_items_category_check,
    ADD CONSTRAINT menu_items_name_check
        CHECK (BTRIM(name) <> '' AND CHAR_LENGTH(name) <= 50),
    ADD CONSTRAINT menu_items_category_check
        CHECK (category IS NULL OR (BTRIM(category) <> '' AND CHAR_LENGTH(category) <= 20));

DROP INDEX IF EXISTS menu_import_jobs_claim_idx;
CREATE INDEX IF NOT EXISTS menu_import_jobs_queued_idx
    ON menu_import_jobs (available_at, created_at) WHERE status = 'queued';
CREATE INDEX IF NOT EXISTS menu_import_jobs_processing_idx
    ON menu_import_jobs (locked_at) WHERE status = 'processing';
