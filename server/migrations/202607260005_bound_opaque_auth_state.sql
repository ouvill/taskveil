-- OPAQUE start state capacity is a database invariant. Both old deployments
-- (which omit identifier_key) and new deployments pass through these triggers,
-- so rolling deployment cannot create unleased state.
ALTER TABLE opaque_registration_states
    ADD COLUMN identifier_key BYTEA
    CHECK (identifier_key IS NULL OR octet_length(identifier_key) = 32);
ALTER TABLE opaque_login_states
    ADD COLUMN identifier_key BYTEA
    CHECK (identifier_key IS NULL OR octet_length(identifier_key) = 32);

CREATE TABLE opaque_state_global_capacity (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    active_count INTEGER NOT NULL CHECK (active_count BETWEEN 0 AND 4096)
);

CREATE TABLE opaque_state_identifier_capacity (
    identifier_key BYTEA PRIMARY KEY CHECK (octet_length(identifier_key) = 32),
    active_count INTEGER NOT NULL CHECK (active_count BETWEEN 1 AND 32)
);

CREATE TABLE opaque_state_capacity_leases (
    state_id UUID PRIMARY KEY,
    state_kind TEXT NOT NULL CHECK (state_kind IN ('registration', 'login')),
    identifier_key BYTEA NOT NULL
        REFERENCES opaque_state_identifier_capacity(identifier_key),
    expires_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX opaque_state_capacity_leases_expires_at_idx
    ON opaque_state_capacity_leases(expires_at);

-- This repository is pre-release. Expire in-flight state at the migration
-- boundary rather than retaining rows without a strict per-identifier key.
DELETE FROM opaque_registration_states;
DELETE FROM opaque_login_states;
INSERT INTO opaque_state_global_capacity (singleton, active_count)
VALUES (TRUE, 0);

CREATE FUNCTION public.taskveil_claim_opaque_state_capacity()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
DECLARE
    capacity_key BYTEA;
    claimed INTEGER;
    state_kind TEXT;
BEGIN
    capacity_key := coalesce(NEW.identifier_key, uuid_send(NEW.id) || uuid_send(NEW.id));
    state_kind := CASE TG_TABLE_NAME
        WHEN 'opaque_registration_states' THEN 'registration'
        WHEN 'opaque_login_states' THEN 'login'
        ELSE NULL
    END;
    IF state_kind IS NULL THEN
        RAISE EXCEPTION USING
            ERRCODE = 'P0001',
            MESSAGE = 'opaque state capacity trigger is attached to an invalid table';
    END IF;

    UPDATE public.opaque_state_global_capacity
    SET active_count = active_count + 1
    WHERE singleton = TRUE AND active_count < 4096
    RETURNING active_count INTO claimed;
    IF claimed IS NULL THEN
        RAISE EXCEPTION USING
            ERRCODE = 'P0429',
            MESSAGE = 'opaque state capacity exhausted';
    END IF;

    claimed := NULL;
    INSERT INTO public.opaque_state_identifier_capacity (identifier_key, active_count)
    VALUES (capacity_key, 1)
    ON CONFLICT (identifier_key) DO UPDATE
    SET active_count = public.opaque_state_identifier_capacity.active_count + 1
    WHERE public.opaque_state_identifier_capacity.active_count < 32
    RETURNING active_count INTO claimed;
    IF claimed IS NULL THEN
        RAISE EXCEPTION USING
            ERRCODE = 'P0429',
            MESSAGE = 'opaque state capacity exhausted';
    END IF;

    INSERT INTO public.opaque_state_capacity_leases
        (state_id, state_kind, identifier_key, expires_at)
    VALUES (NEW.id, state_kind, capacity_key, NEW.expires_at);
    RETURN NEW;
END;
$$;

CREATE FUNCTION public.taskveil_release_opaque_state_capacity()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
DECLARE
    capacity_key BYTEA;
    changed INTEGER;
BEGIN
    DELETE FROM public.opaque_state_capacity_leases
    WHERE state_id = OLD.id
    RETURNING identifier_key INTO capacity_key;
    IF capacity_key IS NULL THEN
        RAISE EXCEPTION USING
            ERRCODE = 'P0001',
            MESSAGE = 'opaque state capacity lease is missing';
    END IF;

    UPDATE public.opaque_state_global_capacity
    SET active_count = active_count - 1
    WHERE singleton = TRUE AND active_count > 0;
    GET DIAGNOSTICS changed = ROW_COUNT;
    IF changed <> 1 THEN
        RAISE EXCEPTION USING
            ERRCODE = 'P0001',
            MESSAGE = 'opaque global capacity underflow';
    END IF;

    UPDATE public.opaque_state_identifier_capacity
    SET active_count = active_count - 1
    WHERE identifier_key = capacity_key AND active_count > 1;
    GET DIAGNOSTICS changed = ROW_COUNT;
    IF changed = 0 THEN
        DELETE FROM public.opaque_state_identifier_capacity
        WHERE identifier_key = capacity_key AND active_count = 1;
        GET DIAGNOSTICS changed = ROW_COUNT;
    END IF;
    IF changed <> 1 THEN
        RAISE EXCEPTION USING
            ERRCODE = 'P0001',
            MESSAGE = 'opaque identifier capacity underflow';
    END IF;
    RETURN OLD;
END;
$$;

REVOKE ALL ON FUNCTION public.taskveil_claim_opaque_state_capacity() FROM PUBLIC;
REVOKE ALL ON FUNCTION public.taskveil_release_opaque_state_capacity() FROM PUBLIC;

CREATE TRIGGER opaque_registration_state_capacity_claim
BEFORE INSERT ON public.opaque_registration_states
FOR EACH ROW EXECUTE FUNCTION public.taskveil_claim_opaque_state_capacity();
CREATE TRIGGER opaque_login_state_capacity_claim
BEFORE INSERT ON public.opaque_login_states
FOR EACH ROW EXECUTE FUNCTION public.taskveil_claim_opaque_state_capacity();
CREATE TRIGGER opaque_registration_state_capacity_release
AFTER DELETE ON public.opaque_registration_states
FOR EACH ROW EXECUTE FUNCTION public.taskveil_release_opaque_state_capacity();
CREATE TRIGGER opaque_login_state_capacity_release
AFTER DELETE ON public.opaque_login_states
FOR EACH ROW EXECUTE FUNCTION public.taskveil_release_opaque_state_capacity();

REVOKE INSERT, UPDATE, DELETE ON opaque_state_global_capacity FROM taskveil_app;
REVOKE INSERT, UPDATE, DELETE ON opaque_state_identifier_capacity FROM taskveil_app;
REVOKE INSERT, UPDATE, DELETE ON opaque_state_capacity_leases FROM taskveil_app;
REVOKE UPDATE ON opaque_registration_states FROM taskveil_app;
REVOKE UPDATE ON opaque_login_states FROM taskveil_app;
GRANT SELECT ON opaque_state_global_capacity TO taskveil_app;
GRANT SELECT ON opaque_state_identifier_capacity TO taskveil_app;
GRANT SELECT ON opaque_state_capacity_leases TO taskveil_app;
