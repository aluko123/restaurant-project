ALTER TABLE inventory_items
    ADD CONSTRAINT inventory_items_restaurant_id_id_key UNIQUE (restaurant_id, id);

ALTER TABLE invoices
    ADD CONSTRAINT invoices_restaurant_id_id_key UNIQUE (restaurant_id, id);

ALTER TABLE invoice_line_items
    ADD CONSTRAINT invoice_line_items_invoice_id_id_key UNIQUE (invoice_id, id);

CREATE TABLE supplier_product_mappings (
    id UUID PRIMARY KEY,
    restaurant_id UUID NOT NULL REFERENCES restaurants(id) ON DELETE CASCADE,
    supplier_name TEXT NOT NULL CHECK (BTRIM(supplier_name) <> '' AND CHAR_LENGTH(supplier_name) <= 120),
    supplier_key TEXT NOT NULL CHECK (BTRIM(supplier_key) <> ''),
    comparison_key TEXT NOT NULL CHECK (BTRIM(comparison_key) <> ''),
    comparison_unit TEXT NOT NULL CHECK (BTRIM(comparison_unit) <> ''),
    product_description TEXT NOT NULL CHECK (BTRIM(product_description) <> '' AND CHAR_LENGTH(product_description) <= 500),
    supplier_sku TEXT CHECK (supplier_sku IS NULL OR CHAR_LENGTH(supplier_sku) <= 120),
    purchase_unit TEXT NOT NULL CHECK (BTRIM(purchase_unit) <> '' AND CHAR_LENGTH(purchase_unit) <= 40),
    inventory_item_id UUID NOT NULL,
    count_units_per_purchase_unit NUMERIC(24, 12) NOT NULL CHECK (count_units_per_purchase_unit > 0),
    created_by UUID NOT NULL REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    FOREIGN KEY (restaurant_id, inventory_item_id)
        REFERENCES inventory_items(restaurant_id, id),
    UNIQUE (restaurant_id, supplier_key, comparison_key, comparison_unit)
);

CREATE INDEX supplier_product_mappings_inventory_item_idx
    ON supplier_product_mappings (restaurant_id, inventory_item_id);

CREATE TABLE purchase_receipts (
    invoice_id UUID PRIMARY KEY,
    restaurant_id UUID NOT NULL,
    supplier_name TEXT NOT NULL CHECK (BTRIM(supplier_name) <> '' AND CHAR_LENGTH(supplier_name) <= 120),
    invoice_number TEXT CHECK (invoice_number IS NULL OR CHAR_LENGTH(invoice_number) <= 120),
    invoice_date DATE NOT NULL,
    currency CHAR(3) NOT NULL,
    recorded_by UUID NOT NULL REFERENCES users(id),
    recorded_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    FOREIGN KEY (restaurant_id, invoice_id)
        REFERENCES invoices(restaurant_id, id) ON DELETE CASCADE,
    UNIQUE (invoice_id, restaurant_id)
);

CREATE INDEX purchase_receipts_restaurant_date_idx
    ON purchase_receipts (restaurant_id, invoice_date DESC, recorded_at DESC, invoice_id DESC);

CREATE TABLE purchase_receipt_lines (
    invoice_id UUID NOT NULL,
    restaurant_id UUID NOT NULL,
    source_line_id UUID NOT NULL,
    position INTEGER NOT NULL CHECK (position >= 0),
    resolution TEXT NOT NULL CHECK (resolution IN ('matched', 'created', 'ignored')),
    supplier_sku TEXT CHECK (supplier_sku IS NULL OR CHAR_LENGTH(supplier_sku) <= 120),
    description TEXT NOT NULL CHECK (BTRIM(description) <> '' AND CHAR_LENGTH(description) <= 500),
    purchase_quantity NUMERIC(18, 6),
    purchase_unit TEXT CHECK (purchase_unit IS NULL OR CHAR_LENGTH(purchase_unit) <= 40),
    unit_price NUMERIC(18, 4),
    line_total NUMERIC(18, 4),
    inventory_item_id UUID,
    inventory_item_name TEXT CHECK (inventory_item_name IS NULL OR (BTRIM(inventory_item_name) <> '' AND CHAR_LENGTH(inventory_item_name) <= 50)),
    count_unit TEXT CHECK (count_unit IS NULL OR (BTRIM(count_unit) <> '' AND CHAR_LENGTH(count_unit) <= 20)),
    count_units_per_purchase_unit NUMERIC(24, 12),
    PRIMARY KEY (invoice_id, source_line_id),
    FOREIGN KEY (invoice_id, restaurant_id)
        REFERENCES purchase_receipts(invoice_id, restaurant_id) ON DELETE CASCADE,
    FOREIGN KEY (invoice_id, source_line_id)
        REFERENCES invoice_line_items(invoice_id, id),
    FOREIGN KEY (restaurant_id, inventory_item_id)
        REFERENCES inventory_items(restaurant_id, id),
    CHECK (
        (resolution = 'ignored'
            AND inventory_item_id IS NULL
            AND inventory_item_name IS NULL
            AND count_unit IS NULL
            AND count_units_per_purchase_unit IS NULL)
        OR
        (resolution IN ('matched', 'created')
            AND inventory_item_id IS NOT NULL
            AND inventory_item_name IS NOT NULL
            AND count_unit IS NOT NULL
            AND purchase_quantity IS NOT NULL
            AND purchase_quantity > 0
            AND purchase_unit IS NOT NULL
            AND BTRIM(purchase_unit) <> ''
            AND count_units_per_purchase_unit IS NOT NULL
            AND count_units_per_purchase_unit > 0)
    )
);

CREATE INDEX purchase_receipt_lines_inventory_item_idx
    ON purchase_receipt_lines (restaurant_id, inventory_item_id)
    WHERE inventory_item_id IS NOT NULL;
