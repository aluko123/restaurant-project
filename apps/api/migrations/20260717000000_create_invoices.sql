CREATE TABLE invoices (
    id UUID PRIMARY KEY,
    restaurant_id UUID NOT NULL REFERENCES restaurants(id) ON DELETE CASCADE,
    uploaded_by UUID NOT NULL REFERENCES users(id),
    supplier_name TEXT NOT NULL CHECK (BTRIM(supplier_name) <> '' AND CHAR_LENGTH(supplier_name) <= 120),
    invoice_date DATE NOT NULL,
    original_filename TEXT NOT NULL CHECK (BTRIM(original_filename) <> '' AND CHAR_LENGTH(original_filename) <= 255),
    content_type TEXT NOT NULL CHECK (CHAR_LENGTH(content_type) <= 100),
    size_bytes BIGINT NOT NULL CHECK (size_bytes > 0 AND size_bytes <= 10485760),
    object_key TEXT NOT NULL UNIQUE CHECK (BTRIM(object_key) <> '' AND CHAR_LENGTH(object_key) <= 500),
    status TEXT NOT NULL DEFAULT 'uploaded' CHECK (
        status IN ('uploaded', 'processing', 'needs_review', 'ready', 'failed')
    ),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX invoices_restaurant_date_idx
    ON invoices (restaurant_id, invoice_date DESC, created_at DESC, id DESC);
