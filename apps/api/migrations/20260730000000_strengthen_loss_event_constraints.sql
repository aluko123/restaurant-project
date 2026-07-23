ALTER TABLE loss_events
    DROP CONSTRAINT loss_events_check,
    ADD CONSTRAINT loss_events_valid_payload_check CHECK (
        (note IS NULL OR (note = BTRIM(note) AND note <> '' AND CHAR_LENGTH(note) <= 500))
        AND (
            (
                event_type = 'waste'
                AND quantity IS NOT NULL
                AND quantity <> 'NaN'::numeric
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
                AND severity IS NOT NULL
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
    );
