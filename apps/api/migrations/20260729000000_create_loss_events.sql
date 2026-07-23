CREATE TABLE loss_events (
    id UUID PRIMARY KEY,
    restaurant_id UUID NOT NULL REFERENCES restaurants(id) ON DELETE CASCADE,
    inventory_item_id UUID NOT NULL,
    created_by UUID NOT NULL REFERENCES users(id),
    event_type TEXT NOT NULL,
    inventory_item_name TEXT NOT NULL,
    count_unit TEXT NOT NULL,
    quantity NUMERIC(18, 6),
    severity TEXT,
    reason TEXT NOT NULL,
    note TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    FOREIGN KEY (restaurant_id, inventory_item_id)
        REFERENCES inventory_items(restaurant_id, id),
    CHECK (
        (note IS NULL OR (note = BTRIM(note) AND note <> '' AND CHAR_LENGTH(note) <= 500))
        AND (
            (
                event_type = 'waste'
                AND quantity > 0
                AND severity IS NULL
                AND reason IN (
                    'spoilage',
                    'overproduction',
                    'prep_mistake',
                    'portioning',
                    'dropped_damaged',
                    'returned',
                    'expired',
                    'other'
                )
            )
            OR (
                event_type = 'stockout'
                AND quantity IS NULL
                AND severity IN ('some_orders', 'menu_item_unavailable', 'service_blocker')
                AND reason IN (
                    'delivery_late_or_missed',
                    'ordered_too_little',
                    'demand_higher_than_expected',
                    'prep_or_portion_issue',
                    'waste_or_spoilage',
                    'other'
                )
            )
        )
    )
);

CREATE INDEX loss_events_restaurant_recent_idx
    ON loss_events (restaurant_id, created_at DESC, id DESC);
