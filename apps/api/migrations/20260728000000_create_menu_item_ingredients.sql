CREATE TABLE menu_item_ingredients (
    id UUID PRIMARY KEY,
    restaurant_id UUID NOT NULL REFERENCES restaurants(id) ON DELETE CASCADE,
    menu_item_id UUID NOT NULL,
    inventory_item_id UUID NOT NULL,
    quantity NUMERIC(18, 6) NOT NULL CHECK (quantity > 0),
    unit TEXT NOT NULL CHECK (unit IN ('g', 'kg', 'oz', 'lb', 'mL', 'L', 'fl_oz_us', 'gal_us', 'each')),
    created_by UUID NOT NULL REFERENCES users(id),
    updated_by UUID NOT NULL REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    FOREIGN KEY (restaurant_id, menu_item_id)
        REFERENCES menu_items(restaurant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (restaurant_id, inventory_item_id)
        REFERENCES inventory_items(restaurant_id, id),
    UNIQUE (menu_item_id, inventory_item_id)
);
