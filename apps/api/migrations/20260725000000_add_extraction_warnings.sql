ALTER TABLE invoice_extractions
    ADD COLUMN has_warnings BOOLEAN NOT NULL DEFAULT FALSE;

ALTER TABLE invoice_line_items
    ADD COLUMN has_warnings BOOLEAN NOT NULL DEFAULT FALSE;

ALTER TABLE menu_import_items
    ADD COLUMN has_warnings BOOLEAN NOT NULL DEFAULT FALSE;
