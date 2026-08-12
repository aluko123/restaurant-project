CREATE TABLE restaurant_setup_streams (
    restaurant_id UUID NOT NULL REFERENCES restaurants(id) ON DELETE CASCADE,
    stream TEXT NOT NULL CHECK (stream IN ('menu','sales','inventory','purchases','bookkeeping_export')),
    method TEXT NOT NULL CHECK (method IN ('connector','import','manual','assisted','deferred')),
    owner TEXT NOT NULL CHECK (owner IN ('restaurant','parline')),
    connector_provider TEXT CHECK (connector_provider IS NULL OR connector_provider = 'square'),
    created_by UUID NOT NULL REFERENCES users(id),
    updated_by UUID NOT NULL REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (restaurant_id, stream),
    CHECK (
        (method = 'connector' AND connector_provider = 'square' AND stream IN ('menu','sales'))
        OR (method <> 'connector' AND connector_provider IS NULL)
    ),
    CHECK (
        (method = 'assisted' AND owner = 'parline')
        OR (method <> 'assisted' AND owner = 'restaurant')
    )
);

ALTER TABLE source_sync_runs
    ADD COLUMN claim_token UUID,
    ADD COLUMN lease_expires_at TIMESTAMPTZ;

UPDATE source_sync_runs SET status='failed',error='Sync interrupted during upgrade.',
    finished_at=NOW()
WHERE status='running';

CREATE UNIQUE INDEX source_sync_runs_one_running_connection_idx
    ON source_sync_runs(connection_id) WHERE status='running';

INSERT INTO restaurant_setup_streams
    (restaurant_id,stream,method,owner,connector_provider,created_by,updated_by)
SELECT restaurant.id,stream.name,
       CASE WHEN restaurant.pos_system='Square' THEN 'connector'
            WHEN restaurant.setup_approach='assisted' THEN 'assisted'
            ELSE 'import' END,
       CASE WHEN restaurant.setup_approach='assisted' AND restaurant.pos_system<>'Square'
            THEN 'parline' ELSE 'restaurant' END,
       CASE WHEN restaurant.pos_system='Square' THEN 'square' END,
       member.user_id,member.user_id
FROM restaurants restaurant
CROSS JOIN (VALUES ('menu'),('sales')) AS stream(name)
JOIN LATERAL (
    SELECT user_id FROM restaurant_memberships
    WHERE restaurant_id=restaurant.id AND role IN ('owner','manager')
    ORDER BY CASE role WHEN 'owner' THEN 0 ELSE 1 END,created_at
    LIMIT 1
) member ON TRUE
WHERE restaurant.pos_system IS NOT NULL;

INSERT INTO restaurant_setup_streams
    (restaurant_id,stream,method,owner,connector_provider,created_by,updated_by)
SELECT connection.restaurant_id,stream.name,'connector','restaurant','square',
       connection.created_by,connection.created_by
FROM source_connections connection
CROSS JOIN (VALUES ('menu'),('sales')) AS stream(name)
WHERE connection.provider='square' AND connection.status<>'disconnected'
ON CONFLICT (restaurant_id,stream) DO UPDATE SET
    method='connector',owner='restaurant',connector_provider='square',
    updated_by=EXCLUDED.updated_by,updated_at=NOW();
