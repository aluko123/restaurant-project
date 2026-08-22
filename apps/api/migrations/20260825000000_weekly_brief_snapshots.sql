-- Weekly brief snapshots: the current-week brief is computed live and stored
-- on every read, so when the week rolls over the last computed version stays
-- available as history.
CREATE TABLE weekly_brief_snapshots (
    restaurant_id UUID NOT NULL REFERENCES restaurants(id) ON DELETE CASCADE,
    week_start DATE NOT NULL,
    payload JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (restaurant_id, week_start)
);
