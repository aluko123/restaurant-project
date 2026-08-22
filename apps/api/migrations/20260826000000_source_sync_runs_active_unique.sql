-- One queued-or-running sync per connection; makes auto-sync dedup
-- race-proof under concurrent workers.

-- Collapse pre-existing pileups: prefer a running row, otherwise the newest.
DELETE FROM source_sync_runs run
WHERE run.status IN ('queued', 'running')
  AND EXISTS (
      SELECT 1
      FROM source_sync_runs other
      WHERE other.connection_id = run.connection_id
        AND other.status IN ('queued', 'running')
        AND (CASE WHEN other.status = 'running' THEN 0 ELSE 1 END,
             other.created_at, other.id)
          < (CASE WHEN run.status = 'running' THEN 0 ELSE 1 END,
             run.created_at, run.id)
  );

CREATE UNIQUE INDEX IF NOT EXISTS source_sync_runs_one_active_idx
    ON source_sync_runs (connection_id)
    WHERE status IN ('queued', 'running');
