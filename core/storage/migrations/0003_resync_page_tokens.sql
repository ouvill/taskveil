-- Protocol v9 replaces raw base cursors with server-authenticated page-token
-- chains. A v8 in-flight generation has no token and cannot be resumed safely.
DELETE FROM sync_full_resync_marks;
DELETE FROM sync_full_resync_state;
