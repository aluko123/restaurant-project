-- Connector seam lifecycle: OAuth completion imports until the first sync
-- succeeds, and menu/sales report their own sync outcomes separately.
ALTER TABLE source_connections DROP CONSTRAINT source_connections_status_check;
ALTER TABLE source_connections ADD CONSTRAINT source_connections_status_check
    CHECK (status IN ('pending','importing','connected','needs_reauth','syncing','error','disconnected'));

ALTER TABLE source_connections
    ADD COLUMN menu_last_success_at TIMESTAMPTZ,
    ADD COLUMN sales_last_success_at TIMESTAMPTZ;

-- Existing connections earned their success on both streams at once.
UPDATE source_connections
SET menu_last_success_at = last_success_at,
    sales_last_success_at = last_success_at
WHERE last_success_at IS NOT NULL;

DROP INDEX source_connections_status_idx;
CREATE INDEX source_connections_status_idx
    ON source_connections (status, updated_at)
    WHERE status IN ('importing','connected','syncing','needs_reauth');
