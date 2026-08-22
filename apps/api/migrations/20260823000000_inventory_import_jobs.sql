-- Inventory CSV extraction moves to the background job pattern used by
-- invoices and menus, so uploads no longer block on Gemini inside the
-- HTTP request.
ALTER TABLE inventory_imports
    ADD COLUMN IF NOT EXISTS object_key TEXT,
    DROP CONSTRAINT IF EXISTS inventory_imports_status_check,
    ADD CONSTRAINT inventory_imports_status_check
        CHECK (status IN ('processing','needs_review','applied','failed')),
    DROP CONSTRAINT IF EXISTS inventory_imports_check,
    ADD CONSTRAINT inventory_imports_lifecycle_check CHECK (
        (status='needs_review' AND applied_by IS NULL AND applied_at IS NULL) OR
        ((status='processing' OR status='failed') AND applied_by IS NULL AND applied_at IS NULL) OR
        (status='applied' AND applied_by IS NOT NULL AND applied_at IS NOT NULL)
    );

CREATE TABLE inventory_import_jobs (
    import_id UUID PRIMARY KEY REFERENCES inventory_imports(id) ON DELETE CASCADE,
    status TEXT NOT NULL DEFAULT 'queued' CHECK (status IN ('queued','processing','completed','failed')),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    available_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    locked_at TIMESTAMPTZ,
    lock_token UUID,
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX inventory_import_jobs_queued_idx
    ON inventory_import_jobs (available_at, created_at) WHERE status = 'queued';
CREATE INDEX inventory_import_jobs_processing_idx
    ON inventory_import_jobs (locked_at) WHERE status = 'processing';

-- Widen the lifecycle guard: extraction now moves processing→needs_review or
-- processing→failed, retries move failed→processing, and failed previews may
-- be discarded just like unreviewed ones.
CREATE OR REPLACE FUNCTION guard_inventory_import() RETURNS TRIGGER AS $$ BEGIN
  IF TG_OP='DELETE' THEN
    IF OLD.status IN ('needs_review','failed') THEN RETURN OLD; END IF;
    RAISE EXCEPTION 'applied imports cannot be deleted';
  END IF;
  IF OLD.status='applied' THEN RAISE EXCEPTION 'applied imports are immutable'; END IF;
  IF NEW.id<>OLD.id OR NEW.restaurant_id<>OLD.restaurant_id OR NEW.content_hash<>OLD.content_hash
     OR NEW.created_by<>OLD.created_by OR NEW.created_at<>OLD.created_at
     OR NEW.object_key IS DISTINCT FROM OLD.object_key THEN
    RAISE EXCEPTION 'invalid import transition';
  END IF;
  IF (OLD.status='needs_review' AND NEW.status='applied'
        AND NEW.revision=OLD.revision+1 AND NEW.applied_by IS NOT NULL AND NEW.applied_at IS NOT NULL)
    OR (OLD.status IN ('processing','failed') AND NEW.status='needs_review'
        AND NEW.revision=OLD.revision AND NEW.applied_by IS NULL AND NEW.applied_at IS NULL)
    OR (OLD.status='processing' AND NEW.status='failed'
        AND NEW.revision=OLD.revision AND NEW.applied_by IS NULL AND NEW.applied_at IS NULL)
    OR (OLD.status='failed' AND NEW.status='processing'
        AND NEW.revision=OLD.revision AND NEW.applied_by IS NULL AND NEW.applied_at IS NULL) THEN
    RETURN NEW;
  END IF;
  RAISE EXCEPTION 'invalid import transition';
END $$ LANGUAGE plpgsql;
