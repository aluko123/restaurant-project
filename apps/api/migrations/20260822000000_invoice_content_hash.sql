ALTER TABLE invoices ADD COLUMN content_hash TEXT;

CREATE UNIQUE INDEX invoices_restaurant_content_hash_idx
    ON invoices(restaurant_id, content_hash)
    WHERE content_hash IS NOT NULL;
