-- Protocol v9 replaces mutable server-side base cursors with authenticated,
-- stateless page-token chains. In-flight v8 sessions and their unacknowledged
-- closure proofs cannot be resumed across this protocol boundary.
DELETE FROM continuity_closure_proofs
WHERE acknowledged_at IS NULL;
DELETE FROM device_resync_sessions;

ALTER TABLE device_resync_sessions
    DROP CONSTRAINT IF EXISTS device_resync_sessions_base_cursor_collection_check;
ALTER TABLE device_resync_sessions
    DROP COLUMN IF EXISTS base_cursor_collection,
    DROP COLUMN IF EXISTS base_cursor_record_id;
