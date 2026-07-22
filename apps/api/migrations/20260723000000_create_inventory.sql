CREATE TABLE inventory_items (
    id UUID PRIMARY KEY,
    restaurant_id UUID NOT NULL REFERENCES restaurants(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (BTRIM(name) <> '' AND CHAR_LENGTH(name) <= 50),
    category TEXT CHECK (category IS NULL OR (BTRIM(category) <> '' AND CHAR_LENGTH(category) <= 20)),
    count_unit TEXT NOT NULL CHECK (BTRIM(count_unit) <> '' AND CHAR_LENGTH(count_unit) <= 20),
    par_level NUMERIC(18,6) CHECK (par_level IS NULL OR par_level >= 0),
    active BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE UNIQUE INDEX inventory_items_restaurant_name_lower_idx
    ON inventory_items (restaurant_id, LOWER(name));
CREATE INDEX inventory_items_restaurant_sort_idx
    ON inventory_items (restaurant_id, active DESC, category, name, id);

CREATE TABLE inventory_count_sessions (
    id UUID PRIMARY KEY,
    restaurant_id UUID NOT NULL REFERENCES restaurants(id) ON DELETE CASCADE,
    created_by UUID NOT NULL REFERENCES users(id),
    status TEXT NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'completed')),
    completed_by UUID REFERENCES users(id),
    completed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CHECK ((status = 'draft' AND completed_by IS NULL AND completed_at IS NULL)
        OR (status = 'completed' AND completed_by IS NOT NULL AND completed_at IS NOT NULL))
);
CREATE UNIQUE INDEX inventory_count_sessions_one_draft_idx
    ON inventory_count_sessions (restaurant_id) WHERE status = 'draft';

CREATE TABLE inventory_count_entries (
    id UUID PRIMARY KEY,
    session_id UUID NOT NULL REFERENCES inventory_count_sessions(id) ON DELETE CASCADE,
    inventory_item_id UUID NOT NULL REFERENCES inventory_items(id),
    name TEXT NOT NULL CHECK (BTRIM(name) <> '' AND CHAR_LENGTH(name) <= 50),
    category TEXT CHECK (category IS NULL OR (BTRIM(category) <> '' AND CHAR_LENGTH(category) <= 20)),
    count_unit TEXT NOT NULL CHECK (BTRIM(count_unit) <> '' AND CHAR_LENGTH(count_unit) <= 20),
    quantity NUMERIC(18,6) CHECK (quantity IS NULL OR quantity >= 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (session_id, inventory_item_id)
);
CREATE INDEX inventory_count_entries_item_idx ON inventory_count_entries (inventory_item_id);

CREATE FUNCTION protect_inventory_completed_session() RETURNS TRIGGER AS $$
BEGIN
    IF OLD.status = 'completed' THEN
        RAISE EXCEPTION 'completed inventory counts are immutable';
    END IF;
    IF TG_OP = 'UPDATE' AND NEW.status = 'completed' THEN
        IF NEW.id <> OLD.id OR NEW.restaurant_id <> OLD.restaurant_id
           OR NEW.created_by <> OLD.created_by OR NEW.created_at <> OLD.created_at THEN
            RAISE EXCEPTION 'only completion fields may change when completing an inventory count';
        END IF;
    END IF;
    RETURN CASE WHEN TG_OP = 'DELETE' THEN OLD ELSE NEW END;
END;
$$ LANGUAGE plpgsql;
CREATE TRIGGER inventory_completed_session_guard
    BEFORE UPDATE OR DELETE ON inventory_count_sessions
    FOR EACH ROW EXECUTE FUNCTION protect_inventory_completed_session();

CREATE FUNCTION protect_inventory_completed_entry() RETURNS TRIGGER AS $$
DECLARE target_session UUID;
BEGIN
    target_session := CASE WHEN TG_OP = 'DELETE' THEN OLD.session_id ELSE NEW.session_id END;
    IF EXISTS (SELECT 1 FROM inventory_count_sessions WHERE id = target_session AND status = 'completed') THEN
        RAISE EXCEPTION 'completed inventory count entries are immutable';
    END IF;
    IF TG_OP = 'UPDATE' AND NEW.session_id <> OLD.session_id THEN
        RAISE EXCEPTION 'inventory count entries cannot move between sessions';
    END IF;
    RETURN CASE WHEN TG_OP = 'DELETE' THEN OLD ELSE NEW END;
END;
$$ LANGUAGE plpgsql;
CREATE TRIGGER inventory_completed_entry_guard
    BEFORE INSERT OR UPDATE OR DELETE ON inventory_count_entries
    FOR EACH ROW EXECUTE FUNCTION protect_inventory_completed_entry();
