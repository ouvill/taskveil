DROP TABLE IF EXISTS sessions;

CREATE TABLE IF NOT EXISTS session_families (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    device_id UUID NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    client_id TEXT NOT NULL CHECK (client_id = 'taskveil-native'),
    absolute_expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    revocation_reason TEXT CHECK (
        revocation_reason IS NULL
        OR revocation_reason IN (
            'client_revocation',
            'refresh_reuse',
            'absolute_expiry',
            'device_revocation',
            'device_key_expiry'
        )
    ),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS session_families_user_device_idx
    ON session_families(user_id, device_id);
CREATE INDEX IF NOT EXISTS session_families_absolute_expires_at_idx
    ON session_families(absolute_expires_at);

CREATE TABLE IF NOT EXISTS access_tokens (
    id UUID PRIMARY KEY,
    family_id UUID NOT NULL REFERENCES session_families(id) ON DELETE CASCADE,
    token_hash BYTEA NOT NULL UNIQUE CHECK (octet_length(token_hash) = 32),
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS access_tokens_family_idx ON access_tokens(family_id);
CREATE INDEX IF NOT EXISTS access_tokens_expires_at_idx ON access_tokens(expires_at);

CREATE TABLE IF NOT EXISTS refresh_tokens (
    id UUID PRIMARY KEY,
    family_id UUID NOT NULL REFERENCES session_families(id) ON DELETE CASCADE,
    generation BIGINT NOT NULL CHECK (generation > 0),
    token_hash BYTEA NOT NULL UNIQUE CHECK (octet_length(token_hash) = 32),
    expires_at TIMESTAMPTZ NOT NULL,
    consumed_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ,
    replaced_by_id UUID REFERENCES refresh_tokens(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (family_id, generation)
);
CREATE INDEX IF NOT EXISTS refresh_tokens_family_idx ON refresh_tokens(family_id);
CREATE INDEX IF NOT EXISTS refresh_tokens_expires_at_idx ON refresh_tokens(expires_at);

GRANT SELECT, INSERT, UPDATE, DELETE ON session_families TO taskveil_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON access_tokens TO taskveil_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON refresh_tokens TO taskveil_app;
