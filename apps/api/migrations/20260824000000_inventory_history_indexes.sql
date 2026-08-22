-- Hot paths (inventory list, Today's below-par actions, order-guide source
-- selection) rank count history per restaurant on every request. Give the
-- planner indexes that match those scans instead of walking every session.
CREATE INDEX IF NOT EXISTS inventory_count_sessions_completed_idx
    ON inventory_count_sessions (restaurant_id, completed_at DESC, id DESC)
    WHERE status = 'completed';

CREATE INDEX IF NOT EXISTS inventory_count_entries_session_item_idx
    ON inventory_count_entries (session_id, inventory_item_id) INCLUDE (quantity);

-- /v1/me checks "do any pending invitations exist?" on every login before
-- deciding to reconcile against WorkOS. Keep that probe index-only.
CREATE INDEX IF NOT EXISTS team_invitations_pending_idx
    ON team_invitations (created_at) WHERE state = 'pending';
