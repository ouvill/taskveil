CREATE TABLE local_profile_runtime (
    singleton INTEGER PRIMARY KEY NOT NULL CHECK (singleton = 1),
    runtime_epoch INTEGER NOT NULL CHECK (runtime_epoch > 0),
    capsule_generation INTEGER NOT NULL CHECK (capsule_generation > 0),
    updated_at INTEGER NOT NULL
);

INSERT INTO local_profile_runtime (
    singleton,
    runtime_epoch,
    capsule_generation,
    updated_at
) VALUES (1, 1, 1, 0);

CREATE TABLE sync_run_lease (
    singleton INTEGER PRIMARY KEY NOT NULL CHECK (singleton = 1),
    owner_id TEXT,
    expires_at_ms INTEGER,
    fencing_token INTEGER NOT NULL CHECK (fencing_token >= 0),
    runtime_epoch INTEGER NOT NULL CHECK (runtime_epoch > 0),
    updated_at INTEGER NOT NULL,
    CHECK (
        (owner_id IS NULL AND expires_at_ms IS NULL)
        OR (
            owner_id IS NOT NULL
            AND length(owner_id) BETWEEN 1 AND 128
            AND expires_at_ms IS NOT NULL
        )
    )
);

INSERT INTO sync_run_lease (
    singleton,
    owner_id,
    expires_at_ms,
    fencing_token,
    runtime_epoch,
    updated_at
) VALUES (1, NULL, NULL, 0, 1, 0);

-- Key rotation can leave records encrypted under older Tenant Root DEK
-- generations. Keep every locally recoverable generation; the greatest
-- generation is the active key and lower generations are decrypt-only.
ALTER TABLE local_tenant_root_key_cache
    RENAME TO local_tenant_root_key_cache_v1;

CREATE TABLE local_tenant_root_key_cache (
    tenant_id TEXT NOT NULL,
    generation INTEGER NOT NULL CHECK (generation > 0),
    wrapped_tenant_root_dek BLOB NOT NULL CHECK (length(wrapped_tenant_root_dek) > 0),
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (tenant_id, generation)
);

INSERT INTO local_tenant_root_key_cache (
    tenant_id,
    generation,
    wrapped_tenant_root_dek,
    updated_at
)
SELECT tenant_id, generation, wrapped_tenant_root_dek, updated_at
FROM local_tenant_root_key_cache_v1;

DROP TABLE local_tenant_root_key_cache_v1;
