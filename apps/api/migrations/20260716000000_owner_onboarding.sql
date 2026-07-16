DROP INDEX users_email_lower_idx;
ALTER TABLE users ALTER COLUMN email DROP NOT NULL;

ALTER TABLE restaurants DROP COLUMN slug;
ALTER TABLE restaurants
    ADD COLUMN city TEXT NOT NULL CHECK (BTRIM(city) <> '' AND CHAR_LENGTH(city) <= 100),
    ADD COLUMN service_style TEXT NOT NULL CHECK (
        service_style IN ('counter_service', 'full_service', 'fast_casual', 'cafe_bakery', 'bar')
    );

DROP INDEX restaurant_memberships_user_id_idx;
ALTER TABLE restaurant_memberships
    ADD CONSTRAINT restaurant_memberships_one_restaurant_per_user UNIQUE (user_id);
