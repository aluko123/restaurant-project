ALTER TABLE invoice_line_items
    ADD COLUMN comparison_key TEXT,
    ADD COLUMN comparison_unit TEXT;

UPDATE invoice_line_items
SET comparison_key = CASE
        WHEN NULLIF(REGEXP_REPLACE(LOWER(BTRIM(COALESCE(sku, ''))), '[^a-z0-9]+', '', 'g'), '') IS NOT NULL
            THEN 'sku:' || REGEXP_REPLACE(LOWER(BTRIM(sku)), '[^a-z0-9]+', '', 'g')
        ELSE NULLIF('description:' || REGEXP_REPLACE(LOWER(BTRIM(description)), '[^a-z0-9]+', '', 'g'), 'description:')
    END,
    comparison_unit = NULLIF(REGEXP_REPLACE(LOWER(BTRIM(COALESCE(unit, ''))), '[^a-z0-9]+', '', 'g'), '');

CREATE INDEX invoice_line_items_comparison_idx
    ON invoice_line_items (invoice_id, comparison_key, comparison_unit)
    WHERE comparison_key IS NOT NULL AND comparison_unit IS NOT NULL AND unit_price > 0;

CREATE INDEX invoices_price_history_idx
    ON invoices (restaurant_id, (LOWER(BTRIM(supplier_name))), invoice_date, created_at, id)
    WHERE status = 'ready';
