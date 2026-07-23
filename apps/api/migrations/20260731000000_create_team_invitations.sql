CREATE TABLE team_invitations (
    id UUID PRIMARY KEY,
    restaurant_id UUID NOT NULL REFERENCES restaurants(id) ON DELETE CASCADE,
    email TEXT NOT NULL CHECK (
        email = LOWER(BTRIM(email)) AND BTRIM(email) <> '' AND CHAR_LENGTH(email) <= 254
    ),
    role TEXT NOT NULL CHECK (role IN ('manager', 'staff')),
    workos_invitation_id TEXT UNIQUE,
    state TEXT NOT NULL CHECK (state IN ('creating', 'pending', 'accepted', 'revoked', 'expired', 'failed')),
    invited_by UUID REFERENCES users(id) ON DELETE SET NULL,
    inviter_auth_subject TEXT NOT NULL CHECK (BTRIM(inviter_auth_subject) <> ''),
    accepted_by UUID REFERENCES users(id),
    provider_expires_at TIMESTAMPTZ,
    accepted_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CHECK ((state IN ('creating', 'failed')) OR workos_invitation_id IS NOT NULL),
    CHECK ((state = 'accepted') = (accepted_by IS NOT NULL)),
    CHECK ((state = 'accepted') = (accepted_at IS NOT NULL)),
    CHECK ((state = 'revoked') = (revoked_at IS NOT NULL))
);

CREATE UNIQUE INDEX team_invitations_one_active_email_idx
    ON team_invitations (LOWER(email)) WHERE state IN ('creating', 'pending');
CREATE INDEX team_invitations_restaurant_active_idx
    ON team_invitations (restaurant_id, created_at, id) WHERE state IN ('creating', 'pending');
CREATE INDEX team_invitations_invited_by_idx ON team_invitations (invited_by);
CREATE INDEX team_invitations_accepted_by_idx ON team_invitations (accepted_by);
