ALTER TABLE restaurants
    ADD COLUMN setup_approach TEXT
        CHECK (setup_approach IS NULL OR setup_approach IN ('assisted', 'self_service'));

ALTER TABLE source_connections
    ADD CONSTRAINT source_connections_restaurant_id_id_key
        UNIQUE (restaurant_id, id);

ALTER TABLE source_sync_runs
    ADD CONSTRAINT source_sync_runs_restaurant_connection_fkey
        FOREIGN KEY (restaurant_id, connection_id)
        REFERENCES source_connections(restaurant_id, id);
