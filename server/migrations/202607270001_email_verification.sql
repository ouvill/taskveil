-- ADR-028 intentionally replaces the pre-release email-bound OPAQUE
-- credential identifier. Existing development account graphs cannot be
-- transformed without the password, so reset them exactly once before adding
-- the stable credential and canonical email invariants.
CREATE TABLE IF NOT EXISTS taskveil_pre_release_resets (
    reset_key TEXT PRIMARY KEY,
    applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
REVOKE ALL PRIVILEGES ON TABLE taskveil_pre_release_resets FROM taskveil_app;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM taskveil_pre_release_resets
        WHERE reset_key = 'email-verification-stable-opaque-credential-v1'
    ) THEN
        -- DELETE, rather than TRUNCATE, preserves the bounded OPAQUE capacity
        -- trigger invariant before the account graph is removed by CASCADE.
        DELETE FROM opaque_registration_states;
        DELETE FROM opaque_login_states;
        TRUNCATE TABLE users CASCADE;
        INSERT INTO taskveil_pre_release_resets (reset_key)
        VALUES ('email-verification-stable-opaque-credential-v1');
    END IF;
END;
$$;

DROP INDEX IF EXISTS users_email_lower_unique;
ALTER TABLE users
    ADD COLUMN IF NOT EXISTS canonical_email TEXT,
    ADD COLUMN IF NOT EXISTS opaque_credential_id UUID;
ALTER TABLE users
    ALTER COLUMN canonical_email SET NOT NULL,
    ALTER COLUMN opaque_credential_id SET NOT NULL;
ALTER TABLE users
    ADD CONSTRAINT users_canonical_email_unique UNIQUE (canonical_email),
    ADD CONSTRAINT users_opaque_credential_id_unique UNIQUE (opaque_credential_id);

ALTER TABLE opaque_registration_states
    DROP COLUMN email,
    ADD COLUMN challenge_id UUID,
    ADD COLUMN opaque_credential_id UUID;

CREATE TABLE email_registration_challenges (
    id UUID PRIMARY KEY,
    canonical_email_digest BYTEA NOT NULL
        CHECK (octet_length(canonical_email_digest) = 36),
    encrypted_registration BYTEA NOT NULL,
    handoff_challenge BYTEA NOT NULL
        CHECK (octet_length(handoff_challenge) = 32),
    opaque_credential_id UUID NOT NULL,
    capacity_claimed BOOLEAN NOT NULL DEFAULT TRUE,
    generation INTEGER NOT NULL DEFAULT 1 CHECK (generation > 0),
    resend_count SMALLINT NOT NULL DEFAULT 0
        CHECK (resend_count BETWEEN 0 AND 3),
    last_delivery_at TIMESTAMPTZ,
    next_retry_at TIMESTAMPTZ NOT NULL,
    otp_digest BYTEA NOT NULL
        CHECK (octet_length(otp_digest) = 36),
    otp_expires_at TIMESTAMPTZ NOT NULL,
    verified_at TIMESTAMPTZ,
    is_decoy BOOLEAN,
    handoff_failed_attempts SMALLINT NOT NULL DEFAULT 0
        CHECK (handoff_failed_attempts BETWEEN 0 AND 8),
    otp_failed_attempts SMALLINT NOT NULL DEFAULT 0
        CHECK (otp_failed_attempts BETWEEN 0 AND 5),
    registration_ticket_ciphertext BYTEA,
    registration_ticket_digest BYTEA UNIQUE
        CHECK (
            registration_ticket_digest IS NULL
            OR octet_length(registration_ticket_digest) = 36
        ),
    ticket_expires_at TIMESTAMPTZ,
    ticket_consumed_at TIMESTAMPTZ,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (
        (verified_at IS NULL
         AND is_decoy IS NULL
         AND registration_ticket_ciphertext IS NULL
         AND registration_ticket_digest IS NULL
         AND ticket_expires_at IS NULL
         AND ticket_consumed_at IS NULL)
        OR
        (verified_at IS NOT NULL
         AND is_decoy IS NOT NULL
         AND registration_ticket_ciphertext IS NOT NULL
         AND registration_ticket_digest IS NOT NULL
         AND ticket_expires_at IS NOT NULL)
    )
);
CREATE INDEX email_registration_challenges_expiry_idx
    ON email_registration_challenges(expires_at, id);
CREATE INDEX email_registration_challenges_canonical_digest_idx
    ON email_registration_challenges(canonical_email_digest, created_at DESC);

-- Provider delivery suppression is durable and shared by every Lambda
-- instance.  It is keyed by the versioned canonical-email digest and accessed
-- under rotation-aware advisory locks, so a new request cannot bypass either
-- the minimum interval or the finite window budget.
CREATE TABLE email_registration_delivery_limits (
    canonical_email_digest BYTEA PRIMARY KEY
        CHECK (octet_length(canonical_email_digest) = 36),
    delivery_count SMALLINT NOT NULL CHECK (delivery_count BETWEEN 1 AND 4),
    last_delivery_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX email_registration_delivery_limits_expiry_idx
    ON email_registration_delivery_limits(expires_at, canonical_email_digest);

CREATE TABLE email_registration_global_capacity (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    active_count INTEGER NOT NULL CHECK (active_count BETWEEN 0 AND 4096)
);
INSERT INTO email_registration_global_capacity (singleton, active_count)
VALUES (TRUE, 0);

CREATE TABLE email_registration_identifier_capacity (
    canonical_email_digest BYTEA PRIMARY KEY
        CHECK (octet_length(canonical_email_digest) = 36),
    active_count SMALLINT NOT NULL CHECK (active_count BETWEEN 1 AND 4)
);

CREATE FUNCTION public.taskveil_claim_email_registration_capacity()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
DECLARE
    claimed INTEGER;
BEGIN
    UPDATE public.email_registration_global_capacity
    SET active_count = active_count + 1
    WHERE singleton = TRUE AND active_count < 4096
    RETURNING active_count INTO claimed;
    IF claimed IS NULL THEN
        RAISE EXCEPTION USING ERRCODE = 'P0429',
            MESSAGE = 'email registration capacity exhausted';
    END IF;

    IF NEW.capacity_claimed THEN
        claimed := NULL;
        INSERT INTO public.email_registration_identifier_capacity
            (canonical_email_digest, active_count)
        VALUES (NEW.canonical_email_digest, 1)
        ON CONFLICT (canonical_email_digest) DO UPDATE
        SET active_count =
            public.email_registration_identifier_capacity.active_count + 1
        WHERE public.email_registration_identifier_capacity.active_count < 4
        RETURNING active_count INTO claimed;
        IF claimed IS NULL THEN
            RAISE EXCEPTION USING ERRCODE = 'P0429',
                MESSAGE = 'email registration identifier capacity exhausted';
        END IF;
    END IF;
    RETURN NEW;
END;
$$;

CREATE FUNCTION public.taskveil_release_email_registration_capacity()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
BEGIN
    UPDATE public.email_registration_global_capacity
    SET active_count = active_count - 1
    WHERE singleton = TRUE AND active_count > 0;
    IF OLD.capacity_claimed THEN
        DELETE FROM public.email_registration_identifier_capacity
        WHERE canonical_email_digest = OLD.canonical_email_digest
          AND active_count = 1;
        IF FOUND THEN
            RETURN OLD;
        END IF;
        UPDATE public.email_registration_identifier_capacity
        SET active_count = active_count - 1
        WHERE canonical_email_digest = OLD.canonical_email_digest
          AND active_count > 1;
    END IF;
    RETURN OLD;
END;
$$;

REVOKE ALL ON FUNCTION public.taskveil_claim_email_registration_capacity() FROM PUBLIC;
REVOKE ALL ON FUNCTION public.taskveil_release_email_registration_capacity() FROM PUBLIC;

CREATE FUNCTION public.taskveil_promote_email_registration_capacity(
    requested_challenge_id UUID
)
RETURNS BOOLEAN
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
DECLARE
    claimed INTEGER;
    stored_digest BYTEA;
    already_claimed BOOLEAN;
BEGIN
    SELECT canonical_email_digest, capacity_claimed
    INTO stored_digest, already_claimed
    FROM public.email_registration_challenges
    WHERE id = requested_challenge_id
    FOR UPDATE;
    IF NOT FOUND OR already_claimed THEN
        RETURN FALSE;
    END IF;

    INSERT INTO public.email_registration_identifier_capacity
        (canonical_email_digest, active_count)
    VALUES (stored_digest, 1)
    ON CONFLICT (canonical_email_digest) DO UPDATE
    SET active_count =
        public.email_registration_identifier_capacity.active_count + 1
    WHERE public.email_registration_identifier_capacity.active_count < 4
    RETURNING active_count INTO claimed;
    IF claimed IS NOT NULL THEN
        UPDATE public.email_registration_challenges
        SET capacity_claimed = TRUE
        WHERE id = requested_challenge_id
          AND capacity_claimed = FALSE;
    END IF;
    RETURN claimed IS NOT NULL;
END;
$$;

REVOKE ALL ON FUNCTION
    public.taskveil_promote_email_registration_capacity(UUID) FROM PUBLIC;
CREATE TRIGGER email_registration_capacity_claim
BEFORE INSERT ON email_registration_challenges
FOR EACH ROW EXECUTE FUNCTION public.taskveil_claim_email_registration_capacity();
CREATE TRIGGER email_registration_capacity_release
AFTER DELETE ON email_registration_challenges
FOR EACH ROW EXECUTE FUNCTION public.taskveil_release_email_registration_capacity();

CREATE TABLE email_registration_reservations (
    canonical_email_digest BYTEA PRIMARY KEY
        CHECK (octet_length(canonical_email_digest) = 36),
    challenge_id UUID NOT NULL UNIQUE
        REFERENCES email_registration_challenges(id) ON DELETE CASCADE,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX email_registration_reservations_expiry_idx
    ON email_registration_reservations(expires_at, challenge_id);

CREATE TABLE email_delivery_outbox (
    challenge_id UUID NOT NULL
        REFERENCES email_registration_challenges(id) ON DELETE CASCADE,
    generation INTEGER NOT NULL CHECK (generation > 0),
    encrypted_command BYTEA,
    not_after TIMESTAMPTZ NOT NULL,
    attempt_count SMALLINT NOT NULL DEFAULT 0
        CHECK (attempt_count BETWEEN 0 AND 12),
    available_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    claim_id UUID,
    claim_expires_at TIMESTAMPTZ,
    accepted_at TIMESTAMPTZ,
    terminal_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (challenge_id, generation),
    CHECK ((claim_id IS NULL) = (claim_expires_at IS NULL)),
    CHECK (NOT (accepted_at IS NOT NULL AND terminal_at IS NOT NULL))
);
CREATE INDEX email_delivery_outbox_dispatch_idx
    ON email_delivery_outbox(available_at, challenge_id, generation)
    WHERE accepted_at IS NULL AND terminal_at IS NULL;

CREATE TABLE registration_start_idempotency (
    challenge_id UUID PRIMARY KEY
        REFERENCES email_registration_challenges(id) ON DELETE CASCADE,
    purpose TEXT NOT NULL CHECK (purpose = 'account_registration'),
    idempotency_key_digest BYTEA NOT NULL
        CHECK (octet_length(idempotency_key_digest) = 32),
    request_hash BYTEA NOT NULL CHECK (octet_length(request_hash) = 32),
    response_ciphertext BYTEA NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX registration_start_idempotency_expiry_idx
    ON registration_start_idempotency(expires_at, challenge_id);

CREATE TABLE registration_request_idempotency (
    challenge_id UUID PRIMARY KEY
        REFERENCES email_registration_challenges(id) ON DELETE CASCADE,
    purpose TEXT NOT NULL CHECK (purpose = 'email_registration_request'),
    idempotency_key_digest BYTEA NOT NULL UNIQUE
        CHECK (octet_length(idempotency_key_digest) = 32),
    request_hash BYTEA NOT NULL CHECK (octet_length(request_hash) = 32),
    response_ciphertext BYTEA NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX registration_request_idempotency_expiry_idx
    ON registration_request_idempotency(expires_at, challenge_id);

CREATE TABLE registration_resend_idempotency (
    idempotency_key_digest BYTEA PRIMARY KEY
        CHECK (octet_length(idempotency_key_digest) = 32),
    challenge_id UUID NOT NULL
        REFERENCES email_registration_challenges(id) ON DELETE CASCADE,
    purpose TEXT NOT NULL CHECK (purpose = 'email_registration_resend'),
    request_hash BYTEA NOT NULL CHECK (octet_length(request_hash) = 32),
    response_ciphertext BYTEA NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX registration_resend_idempotency_expiry_idx
    ON registration_resend_idempotency(expires_at, challenge_id);

CREATE TABLE registration_verify_idempotency (
    challenge_id UUID NOT NULL
        REFERENCES email_registration_challenges(id) ON DELETE CASCADE,
    purpose TEXT NOT NULL CHECK (purpose = 'email_registration_verify'),
    idempotency_key_digest BYTEA PRIMARY KEY
        CHECK (octet_length(idempotency_key_digest) = 32),
    request_hash BYTEA NOT NULL CHECK (octet_length(request_hash) = 32),
    response_ciphertext BYTEA NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX registration_verify_idempotency_expiry_idx
    ON registration_verify_idempotency(expires_at, challenge_id);

CREATE TABLE registration_finish_idempotency (
    state_id UUID PRIMARY KEY,
    challenge_id UUID NOT NULL
        REFERENCES email_registration_challenges(id) ON DELETE CASCADE,
    purpose TEXT NOT NULL CHECK (purpose = 'account_registration_finish'),
    idempotency_key_digest BYTEA NOT NULL UNIQUE
        CHECK (octet_length(idempotency_key_digest) = 32),
    request_hash BYTEA NOT NULL CHECK (octet_length(request_hash) = 32),
    response_ciphertext BYTEA NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX registration_finish_idempotency_expiry_idx
    ON registration_finish_idempotency(expires_at, state_id);

CREATE TABLE registration_reconciliation_receipts (
    challenge_id UUID PRIMARY KEY,
    handoff_challenge BYTEA NOT NULL
        CHECK (octet_length(handoff_challenge) = 32),
    state_id UUID NOT NULL,
    finish_idempotency_key_digest BYTEA NOT NULL
        CHECK (octet_length(finish_idempotency_key_digest) = 32),
    request_hash BYTEA NOT NULL CHECK (octet_length(request_hash) = 32),
    response_ciphertext BYTEA NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX registration_reconciliation_receipts_expiry_idx
    ON registration_reconciliation_receipts(expires_at, challenge_id);

CREATE TABLE registration_reconciliation_authorizations (
    challenge_id UUID PRIMARY KEY,
    handoff_challenge BYTEA NOT NULL
        CHECK (octet_length(handoff_challenge) = 32),
    start_idempotency_key_digest BYTEA NOT NULL
        CHECK (octet_length(start_idempotency_key_digest) = 32),
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX registration_reconciliation_authorizations_expiry_idx
    ON registration_reconciliation_authorizations(expires_at, challenge_id);

ALTER TABLE opaque_registration_states
    ALTER COLUMN challenge_id SET NOT NULL,
    ALTER COLUMN opaque_credential_id SET NOT NULL,
    ADD CONSTRAINT opaque_registration_states_challenge_unique
        UNIQUE (challenge_id),
    ADD CONSTRAINT opaque_registration_states_challenge_fk
        FOREIGN KEY (challenge_id)
        REFERENCES email_registration_challenges(id) ON DELETE CASCADE;

GRANT SELECT, INSERT, UPDATE, DELETE ON email_registration_challenges TO taskveil_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON email_registration_delivery_limits TO taskveil_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON email_registration_reservations TO taskveil_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON email_delivery_outbox TO taskveil_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON registration_start_idempotency TO taskveil_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON registration_request_idempotency TO taskveil_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON registration_resend_idempotency TO taskveil_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON registration_verify_idempotency TO taskveil_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON registration_finish_idempotency TO taskveil_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON registration_reconciliation_receipts TO taskveil_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON registration_reconciliation_authorizations TO taskveil_app;
GRANT SELECT ON email_registration_global_capacity TO taskveil_app;
GRANT SELECT ON email_registration_identifier_capacity TO taskveil_app;
GRANT EXECUTE ON FUNCTION
    public.taskveil_promote_email_registration_capacity(UUID) TO taskveil_app;
REVOKE INSERT, UPDATE, DELETE ON email_registration_global_capacity FROM taskveil_app;
REVOKE INSERT, UPDATE, DELETE ON email_registration_identifier_capacity FROM taskveil_app;
