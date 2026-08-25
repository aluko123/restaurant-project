-- Clover can own the menu+sales streams like Square.
ALTER TABLE restaurant_setup_streams
    DROP CONSTRAINT restaurant_setup_streams_connector_provider_check,
    ADD CONSTRAINT restaurant_setup_streams_connector_provider_check
        CHECK (connector_provider IS NULL OR connector_provider IN ('square','clover')),
    DROP CONSTRAINT restaurant_setup_streams_check,
    ADD CONSTRAINT restaurant_setup_streams_check CHECK (
        (
            method = 'connector' AND connector_provider IN ('square','clover')
            AND stream IN ('menu','sales')
        )
        OR (method <> 'connector' AND connector_provider IS NULL)
    );
