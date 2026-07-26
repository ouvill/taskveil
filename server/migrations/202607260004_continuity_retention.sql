-- Fail instead of waiting indefinitely for schema locks. Large production
-- tables use the staged concurrent-index path in the DB migration runbook.
SET LOCAL lock_timeout = '5s';

-- A closure proof is current device state, not an append-only audit log.
-- Preserve an in-flight proof when one exists; otherwise retain the newest
-- acknowledged proof so an ACK response can be replayed idempotently.
WITH ranked AS (
    SELECT proof_id,
           row_number() OVER (
               PARTITION BY tenant_id, device_id
               ORDER BY (acknowledged_at IS NULL) DESC, created_at DESC, proof_id DESC
           ) AS retention_rank
    FROM continuity_closure_proofs
)
DELETE FROM continuity_closure_proofs AS proof
USING ranked
WHERE proof.proof_id = ranked.proof_id
  AND ranked.retention_rank > 1;

DROP INDEX IF EXISTS continuity_closure_proofs_device_idx;
CREATE UNIQUE INDEX IF NOT EXISTS continuity_closure_proofs_current_device_idx
    ON continuity_closure_proofs(tenant_id, device_id);

-- Protocol v9 already replaces sessions on restart. Compact databases created
-- before that behavior, preferring required_generation when it is present and
-- otherwise the greatest generation that could still be inspected.
WITH ranked AS (
    SELECT session.tenant_id,
           session.device_id,
           session.generation,
           row_number() OVER (
               PARTITION BY session.tenant_id, session.device_id
               ORDER BY
                   (session.generation = continuity.required_generation) DESC,
                   session.generation DESC,
                   session.updated_at DESC
           ) AS retention_rank
    FROM device_resync_sessions AS session
    JOIN tenant_device_continuity AS continuity
      ON continuity.tenant_id = session.tenant_id
     AND continuity.device_id = session.device_id
)
DELETE FROM device_resync_sessions AS session
USING ranked
WHERE session.tenant_id = ranked.tenant_id
  AND session.device_id = ranked.device_id
  AND session.generation = ranked.generation
  AND ranked.retention_rank > 1;

CREATE UNIQUE INDEX IF NOT EXISTS device_resync_sessions_current_device_idx
    ON device_resync_sessions(tenant_id, device_id);
