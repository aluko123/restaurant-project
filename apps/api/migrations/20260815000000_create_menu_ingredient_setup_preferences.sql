CREATE TABLE menu_ingredient_setup_preferences (
    restaurant_id UUID NOT NULL REFERENCES restaurants(id) ON DELETE CASCADE,
    menu_item_id UUID NOT NULL,
    choice TEXT NOT NULL CHECK (choice IN ('important', 'later')),
    created_by UUID NOT NULL REFERENCES users(id),
    updated_by UUID NOT NULL REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (restaurant_id, menu_item_id),
    FOREIGN KEY (restaurant_id, menu_item_id)
        REFERENCES menu_items(restaurant_id, id) ON DELETE CASCADE
);
