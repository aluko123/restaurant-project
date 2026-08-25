-- Reusable count templates: named saved item lists that seed focused counts.
CREATE TABLE count_templates (
    id UUID PRIMARY KEY,
    restaurant_id UUID NOT NULL REFERENCES restaurants(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (BTRIM(name) <> '' AND CHAR_LENGTH(name) <= 60),
    created_by UUID NOT NULL REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX count_templates_restaurant_name_lower_idx
    ON count_templates (restaurant_id, LOWER(BTRIM(name)));

CREATE INDEX count_templates_restaurant_idx
    ON count_templates (restaurant_id, created_at DESC, id DESC);

CREATE TABLE count_template_items (
    template_id UUID NOT NULL REFERENCES count_templates(id) ON DELETE CASCADE,
    inventory_item_id UUID NOT NULL REFERENCES inventory_items(id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position >= 0),
    PRIMARY KEY (template_id, inventory_item_id)
);
