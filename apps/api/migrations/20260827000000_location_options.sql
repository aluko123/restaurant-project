-- Shared location options: the original curated catalog, plus every city
-- any restaurant adds through the "Other" flows afterwards.
CREATE TABLE location_options (
    id UUID PRIMARY KEY,
    country TEXT NOT NULL CHECK (BTRIM(country) <> '' AND CHAR_LENGTH(country) <= 100),
    region TEXT NOT NULL CHECK (BTRIM(region) <> '' AND CHAR_LENGTH(region) <= 100),
    city TEXT NOT NULL CHECK (BTRIM(city) <> '' AND CHAR_LENGTH(city) <= 100),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE UNIQUE INDEX location_options_triple_idx
    ON location_options (LOWER(country), LOWER(region), LOWER(city));

INSERT INTO location_options (id, country, region, city)
VALUES
    (gen_random_uuid(), 'United States', 'Georgia', 'Atlanta'),
    (gen_random_uuid(), 'United States', 'Texas', 'Austin'),
    (gen_random_uuid(), 'United States', 'Maryland', 'Baltimore'),
    (gen_random_uuid(), 'United States', 'Massachusetts', 'Boston'),
    (gen_random_uuid(), 'United States', 'North Carolina', 'Charlotte'),
    (gen_random_uuid(), 'United States', 'Illinois', 'Chicago'),
    (gen_random_uuid(), 'United States', 'Texas', 'Dallas'),
    (gen_random_uuid(), 'United States', 'Colorado', 'Denver'),
    (gen_random_uuid(), 'United States', 'Michigan', 'Detroit'),
    (gen_random_uuid(), 'United States', 'Texas', 'Houston'),
    (gen_random_uuid(), 'United States', 'Nevada', 'Las Vegas'),
    (gen_random_uuid(), 'United States', 'California', 'Los Angeles'),
    (gen_random_uuid(), 'United States', 'Florida', 'Miami'),
    (gen_random_uuid(), 'United States', 'Minnesota', 'Minneapolis'),
    (gen_random_uuid(), 'United States', 'Tennessee', 'Nashville'),
    (gen_random_uuid(), 'United States', 'Louisiana', 'New Orleans'),
    (gen_random_uuid(), 'United States', 'New York', 'New York City'),
    (gen_random_uuid(), 'United States', 'Florida', 'Orlando'),
    (gen_random_uuid(), 'United States', 'Pennsylvania', 'Philadelphia'),
    (gen_random_uuid(), 'United States', 'Arizona', 'Phoenix'),
    (gen_random_uuid(), 'United States', 'Oregon', 'Portland'),
    (gen_random_uuid(), 'United States', 'Texas', 'San Antonio'),
    (gen_random_uuid(), 'United States', 'California', 'San Diego'),
    (gen_random_uuid(), 'United States', 'California', 'San Francisco'),
    (gen_random_uuid(), 'United States', 'Washington', 'Seattle'),
    (gen_random_uuid(), 'United States', 'Missouri', 'St. Louis'),
    (gen_random_uuid(), 'United States', 'Florida', 'Tampa'),
    (gen_random_uuid(), 'United States', 'District of Columbia', 'Washington'),
    (gen_random_uuid(), 'Netherlands', 'North Holland', 'Amsterdam'),
    (gen_random_uuid(), 'Spain', 'Catalonia', 'Barcelona'),
    (gen_random_uuid(), 'Germany', 'Berlin', 'Berlin'),
    (gen_random_uuid(), 'United Arab Emirates', 'Dubai', 'Dubai'),
    (gen_random_uuid(), 'Ireland', 'Leinster', 'Dublin'),
    (gen_random_uuid(), 'Hong Kong', 'Hong Kong', 'Hong Kong'),
    (gen_random_uuid(), 'Nigeria', 'Lagos', 'Lagos'),
    (gen_random_uuid(), 'United Kingdom', 'England', 'London'),
    (gen_random_uuid(), 'Spain', 'Community of Madrid', 'Madrid'),
    (gen_random_uuid(), 'Mexico', 'Mexico City', 'Mexico City'),
    (gen_random_uuid(), 'Canada', 'Quebec', 'Montreal'),
    (gen_random_uuid(), 'France', 'Île-de-France', 'Paris'),
    (gen_random_uuid(), 'Italy', 'Lazio', 'Rome'),
    (gen_random_uuid(), 'Singapore', 'Singapore', 'Singapore'),
    (gen_random_uuid(), 'Australia', 'New South Wales', 'Sydney'),
    (gen_random_uuid(), 'Japan', 'Tokyo', 'Tokyo'),
    (gen_random_uuid(), 'Canada', 'Ontario', 'Toronto'),
    (gen_random_uuid(), 'Canada', 'British Columbia', 'Vancouver')
ON CONFLICT DO NOTHING;
