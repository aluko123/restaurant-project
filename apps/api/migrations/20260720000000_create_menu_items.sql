CREATE TABLE menu_items (
    id UUID PRIMARY KEY,
    restaurant_id UUID NOT NULL REFERENCES restaurants(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (BTRIM(name) <> '' AND CHAR_LENGTH(name) <= 120),
    category TEXT CHECK (category IS NULL OR (BTRIM(category) <> '' AND CHAR_LENGTH(category) <= 60)),
    selling_price NUMERIC(18, 4) NOT NULL CHECK (selling_price > 0),
    currency CHAR(3) NOT NULL CHECK (currency ~ '^[A-Z]{3}$'),
    active BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX menu_items_restaurant_name_lower_idx
    ON menu_items (restaurant_id, LOWER(name));

CREATE INDEX menu_items_restaurant_active_idx
    ON menu_items (restaurant_id, active, created_at, id);
