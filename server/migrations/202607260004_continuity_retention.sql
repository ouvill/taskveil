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
CREATE UNIQUE INDEX continuity_closure_proofs_current_device_idx
    ON continuity_closure_proofs(tenant_id, device_id);
