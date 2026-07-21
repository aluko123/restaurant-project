CREATE TABLE menu_imports (
    id UUID PRIMARY KEY,
    restaurant_id UUID NOT NULL REFERENCES restaurants(id) ON DELETE CASCADE,
    uploaded_by UUID NOT NULL REFERENCES users(id),
    original_filename TEXT NOT NULL CHECK (BTRIM(original_filename) <> '' AND CHAR_LENGTH(original_filename) <= 255),
    content_type TEXT NOT NULL,
    size_bytes BIGINT NOT NULL CHECK (size_bytes > 0 AND size_bytes <= 10485760),
    object_key TEXT NOT NULL UNIQUE,
    status TEXT NOT NULL DEFAULT 'processing' CHECK (status IN ('processing','needs_review','imported','failed')),
    provider TEXT, model_id TEXT, raw_provider_json JSONB,
    prompt_tokens BIGINT, candidate_tokens BIGINT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(), updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX menu_imports_restaurant_idx ON menu_imports (restaurant_id, created_at DESC, id DESC);

CREATE TABLE menu_import_jobs (
    menu_import_id UUID PRIMARY KEY REFERENCES menu_imports(id) ON DELETE CASCADE,
    status TEXT NOT NULL DEFAULT 'queued' CHECK (status IN ('queued','processing','completed','failed')),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0), available_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    locked_at TIMESTAMPTZ, lock_token UUID, last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(), updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX menu_import_jobs_queued_idx ON menu_import_jobs (available_at, created_at) WHERE status = 'queued';
CREATE INDEX menu_import_jobs_processing_idx ON menu_import_jobs (locked_at) WHERE status = 'processing';

CREATE TABLE menu_import_items (
    id UUID PRIMARY KEY, menu_import_id UUID NOT NULL REFERENCES menu_imports(id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position >= 0), name TEXT NOT NULL,
    category TEXT, selling_price NUMERIC(18,4), currency CHAR(3),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(), UNIQUE(menu_import_id, position)
);
