-- Storage areas for walk-order counts
CREATE TABLE storage_areas (
    id UUID PRIMARY KEY,
    restaurant_id UUID NOT NULL REFERENCES restaurants(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (BTRIM(name) <> '' AND CHAR_LENGTH(name) <= 40),
    sort_order INT NOT NULL DEFAULT 0,
    active BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE UNIQUE INDEX storage_areas_restaurant_name_lower_idx
    ON storage_areas (restaurant_id, LOWER(name));
CREATE INDEX storage_areas_restaurant_sort_idx
    ON storage_areas (restaurant_id, active DESC, sort_order, name, id);

ALTER TABLE inventory_items
    ADD COLUMN storage_area_id UUID REFERENCES storage_areas(id) ON DELETE SET NULL,
    ADD COLUMN shelf_order INT NOT NULL DEFAULT 0;
CREATE INDEX inventory_items_placement_idx
    ON inventory_items (restaurant_id, storage_area_id, shelf_order, name, id);

ALTER TABLE inventory_count_sessions
    ADD COLUMN scope TEXT NOT NULL DEFAULT 'all'
        CHECK (scope IN ('all', 'areas'));

CREATE TABLE inventory_count_session_areas (
    session_id UUID NOT NULL REFERENCES inventory_count_sessions(id) ON DELETE CASCADE,
    storage_area_id UUID NOT NULL REFERENCES storage_areas(id) ON DELETE RESTRICT,
    PRIMARY KEY (session_id, storage_area_id)
);

ALTER TABLE inventory_count_entries
    ADD COLUMN storage_area_name TEXT
        CHECK (storage_area_name IS NULL OR (BTRIM(storage_area_name) <> '' AND CHAR_LENGTH(storage_area_name) <= 40)),
    ADD COLUMN storage_area_sort INT NOT NULL DEFAULT 0,
    ADD COLUMN shelf_order INT NOT NULL DEFAULT 0,
    ADD COLUMN previous_quantity NUMERIC(18,6)
        CHECK (previous_quantity IS NULL OR previous_quantity >= 0),
    ADD COLUMN skipped BOOLEAN NOT NULL DEFAULT FALSE;

-- Quantity and skipped are mutually exclusive once set
ALTER TABLE inventory_count_entries
    ADD CONSTRAINT inventory_count_entries_qty_skip_chk
    CHECK (quantity IS NULL OR skipped = FALSE);
