-- Phase 5: durable source connections (Square first) + external IDs for menu/sales.

CREATE TABLE source_connections (
    id UUID PRIMARY KEY,
    restaurant_id UUID NOT NULL REFERENCES restaurants(id) ON DELETE CASCADE,
    provider TEXT NOT NULL CHECK (provider IN ('square')),
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending','connected','needs_reauth','syncing','error','disconnected')),
    external_merchant_id TEXT,
    external_location_id TEXT,
    access_token_encrypted TEXT,
    refresh_token_encrypted TEXT,
    access_token_expires_at TIMESTAMPTZ,
    scopes TEXT,
    last_sync_at TIMESTAMPTZ,
    last_success_at TIMESTAMPTZ,
    last_error TEXT,
    created_by UUID NOT NULL REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (restaurant_id, provider)
);

CREATE INDEX source_connections_status_idx
    ON source_connections (status, updated_at)
    WHERE status IN ('connected','syncing','needs_reauth');

CREATE TABLE source_sync_runs (
    id UUID PRIMARY KEY,
    connection_id UUID NOT NULL REFERENCES source_connections(id) ON DELETE CASCADE,
    restaurant_id UUID NOT NULL REFERENCES restaurants(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('full','incremental')),
    status TEXT NOT NULL DEFAULT 'queued'
        CHECK (status IN ('queued','running','succeeded','failed')),
    stats JSONB NOT NULL DEFAULT '{}'::jsonb,
    error TEXT,
    started_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX source_sync_runs_claim_idx
    ON source_sync_runs (created_at)
    WHERE status = 'queued';

CREATE TABLE oauth_states (
    state TEXT PRIMARY KEY,
    restaurant_id UUID NOT NULL REFERENCES restaurants(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    provider TEXT NOT NULL CHECK (provider IN ('square')),
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX oauth_states_expires_idx ON oauth_states (expires_at);

ALTER TABLE menu_items
    ADD COLUMN external_source TEXT
        CHECK (external_source IS NULL OR external_source IN ('square')),
    ADD COLUMN external_id TEXT
        CHECK (external_id IS NULL OR (BTRIM(external_id) <> '' AND CHAR_LENGTH(external_id) <= 120));

CREATE UNIQUE INDEX menu_items_external_uidx
    ON menu_items (restaurant_id, external_source, external_id)
    WHERE external_source IS NOT NULL AND external_id IS NOT NULL;

ALTER TABLE sales_days
    ADD COLUMN external_source TEXT
        CHECK (external_source IS NULL OR external_source IN ('square'));
