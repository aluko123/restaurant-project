-- Clover joins Square as a connector provider.
ALTER TABLE menu_items DROP CONSTRAINT menu_items_external_source_check,
    ADD CONSTRAINT menu_items_external_source_check
        CHECK (external_source IS NULL OR external_source IN ('square','clover'));
ALTER TABLE sales_days DROP CONSTRAINT sales_days_external_source_check,
    ADD CONSTRAINT sales_days_external_source_check
        CHECK (external_source IS NULL OR external_source IN ('square','clover'));

ALTER TABLE source_connections DROP CONSTRAINT source_connections_provider_check,
    ADD CONSTRAINT source_connections_provider_check
        CHECK (provider IN ('square','clover'));

ALTER TABLE oauth_states DROP CONSTRAINT oauth_states_provider_check,
    ADD CONSTRAINT oauth_states_provider_check
        CHECK (provider IN ('square','clover'));
