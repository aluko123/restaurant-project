ALTER TABLE menu_items
    ADD CONSTRAINT menu_items_restaurant_id_id_key UNIQUE (restaurant_id, id);

CREATE TABLE sales_days (
    id UUID PRIMARY KEY,
    restaurant_id UUID NOT NULL REFERENCES restaurants(id) ON DELETE CASCADE,
    business_date DATE NOT NULL,
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision >= 1),
    created_by UUID NOT NULL REFERENCES users(id),
    updated_by UUID NOT NULL REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (restaurant_id, business_date),
    UNIQUE (restaurant_id, id)
);

CREATE INDEX sales_days_restaurant_recent_idx
    ON sales_days (restaurant_id, business_date DESC, updated_at DESC, id DESC);

CREATE TABLE sales_lines (
    sales_day_id UUID NOT NULL,
    restaurant_id UUID NOT NULL,
    menu_item_id UUID NOT NULL,
    menu_item_name TEXT NOT NULL
        CHECK (BTRIM(menu_item_name) <> '' AND CHAR_LENGTH(menu_item_name) <= 50),
    quantity NUMERIC(18, 6) NOT NULL CHECK (quantity > 0),
    reported_net_sales NUMERIC(18, 4) CHECK (reported_net_sales >= 0),
    currency CHAR(3) CHECK (currency IS NULL OR currency ~ '^[A-Z]{3}$'),
    PRIMARY KEY (sales_day_id, menu_item_id),
    FOREIGN KEY (restaurant_id, sales_day_id)
        REFERENCES sales_days(restaurant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (restaurant_id, menu_item_id)
        REFERENCES menu_items(restaurant_id, id),
    CHECK (
        (reported_net_sales IS NULL AND currency IS NULL)
        OR (reported_net_sales IS NOT NULL AND currency IS NOT NULL)
    )
);

CREATE INDEX sales_lines_restaurant_menu_item_idx
    ON sales_lines (restaurant_id, menu_item_id);
