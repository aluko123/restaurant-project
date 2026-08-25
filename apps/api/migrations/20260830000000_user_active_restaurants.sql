-- Multi-restaurant users work in silos: one login, one active workspace at a
-- time. The preference pins every membership lookup to a deterministic
-- restaurant; without it, the oldest membership wins.
CREATE TABLE user_active_restaurants (
    user_id UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    restaurant_id UUID NOT NULL REFERENCES restaurants(id) ON DELETE CASCADE,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
