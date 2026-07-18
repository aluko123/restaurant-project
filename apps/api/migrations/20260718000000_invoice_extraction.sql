CREATE TABLE invoice_extraction_jobs (
    invoice_id UUID PRIMARY KEY REFERENCES invoices(id) ON DELETE CASCADE,
    status TEXT NOT NULL DEFAULT 'queued' CHECK (status IN ('queued', 'processing', 'completed', 'failed')),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    available_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    locked_at TIMESTAMPTZ,
    lock_token UUID,
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX invoice_extraction_jobs_claim_idx
    ON invoice_extraction_jobs (available_at, created_at)
    WHERE status IN ('queued', 'processing');

CREATE TABLE invoice_extractions (
    invoice_id UUID PRIMARY KEY REFERENCES invoices(id) ON DELETE CASCADE,
    provider TEXT NOT NULL,
    model_id TEXT NOT NULL,
    raw_provider_json JSONB NOT NULL,
    supplier_name TEXT NOT NULL,
    invoice_number TEXT,
    invoice_date DATE,
    currency CHAR(3) NOT NULL,
    subtotal NUMERIC(18, 4),
    tax NUMERIC(18, 4),
    fees NUMERIC(18, 4),
    discount NUMERIC(18, 4),
    total NUMERIC(18, 4),
    prompt_tokens BIGINT,
    candidate_tokens BIGINT,
    reviewed_by UUID REFERENCES users(id),
    reviewed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE invoice_line_items (
    id UUID PRIMARY KEY,
    invoice_id UUID NOT NULL REFERENCES invoices(id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position >= 0),
    sku TEXT,
    description TEXT NOT NULL,
    quantity NUMERIC(18, 6),
    unit TEXT,
    unit_price NUMERIC(18, 4),
    line_total NUMERIC(18, 4),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (invoice_id, position)
);

CREATE INDEX invoice_line_items_invoice_idx ON invoice_line_items (invoice_id, position);

INSERT INTO invoice_extraction_jobs (invoice_id)
SELECT id FROM invoices WHERE status = 'uploaded'
ON CONFLICT (invoice_id) DO NOTHING;

UPDATE invoices SET status = 'processing', updated_at = NOW() WHERE status = 'uploaded';
