-- Clover can own the menu+sales streams like Square.
-- Idempotent: safe to re-run on databases where it was applied out-of-band.
ALTER TABLE restaurant_setup_streams
    DROP CONSTRAINT IF EXISTS restaurant_setup_streams_connector_provider_check;
ALTER TABLE restaurant_setup_streams
    ADD CONSTRAINT restaurant_setup_streams_connector_provider_check
        CHECK (connector_provider IS NULL OR connector_provider IN ('square','clover'));
ALTER TABLE restaurant_setup_streams
    DROP CONSTRAINT IF EXISTS restaurant_setup_streams_check;
ALTER TABLE restaurant_setup_streams
    ADD CONSTRAINT restaurant_setup_streams_check CHECK (
        (
            method = 'connector' AND connector_provider IN ('square','clover')
            AND stream IN ('menu','sales')
        )
        OR (method <> 'connector' AND connector_provider IS NULL)
    );
