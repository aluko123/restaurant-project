ALTER TABLE invoices
    ADD CONSTRAINT invoices_id_restaurant_unique UNIQUE (id, restaurant_id);

ALTER TABLE invoice_line_items
    ADD CONSTRAINT invoice_line_items_id_invoice_unique UNIQUE (id, invoice_id);

CREATE TABLE invoice_price_findings (
    id UUID PRIMARY KEY,
    restaurant_id UUID NOT NULL REFERENCES restaurants(id) ON DELETE CASCADE,
    invoice_id UUID NOT NULL,
    source_line_id UUID NOT NULL,
    supplier_name TEXT NOT NULL,
    invoice_date DATE NOT NULL,
    invoice_created_at TIMESTAMPTZ NOT NULL,
    description TEXT NOT NULL,
    unit TEXT,
    currency TEXT NOT NULL,
    previous_unit_price NUMERIC(18,4) NOT NULL CHECK (previous_unit_price > 0),
    current_unit_price NUMERIC(18,4) NOT NULL CHECK (current_unit_price > 0),
    percentage_change NUMERIC(12,2) NOT NULL,
    previous_invoice_date DATE NOT NULL,
    comparison_key TEXT NOT NULL,
    comparison_unit TEXT NOT NULL,
    increased BOOLEAN NOT NULL,
    at_least_ten_percent BOOLEAN NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('open', 'reviewed', 'baseline')),
    reviewed_by UUID,
    reviewed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT invoice_price_findings_invoice_tenant_fk
        FOREIGN KEY (invoice_id, restaurant_id)
        REFERENCES invoices(id, restaurant_id) ON DELETE CASCADE,
    CONSTRAINT invoice_price_findings_source_line_fk
        FOREIGN KEY (source_line_id, invoice_id)
        REFERENCES invoice_line_items(id, invoice_id) ON DELETE CASCADE,
    CONSTRAINT invoice_price_findings_reviewer_fk
        FOREIGN KEY (restaurant_id, reviewed_by)
        REFERENCES restaurant_memberships(restaurant_id, user_id)
        ON DELETE SET NULL (reviewed_by),
    CONSTRAINT invoice_price_findings_review_audit_check CHECK (
        (status = 'reviewed' AND reviewed_at IS NOT NULL)
        OR (status <> 'reviewed' AND reviewed_by IS NULL AND reviewed_at IS NULL)
    ),
    UNIQUE (restaurant_id, source_line_id)
);

CREATE INDEX invoice_price_findings_invoice_idx
    ON invoice_price_findings (restaurant_id, invoice_id, status);

CREATE INDEX invoice_price_findings_open_idx
    ON invoice_price_findings (restaurant_id, invoice_created_at DESC, id)
    WHERE status = 'open';
