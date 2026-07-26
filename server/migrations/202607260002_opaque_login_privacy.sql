ALTER TABLE opaque_login_states
    ALTER COLUMN user_id DROP NOT NULL,
    ALTER COLUMN tenant_id DROP NOT NULL;

ALTER TABLE opaque_login_states
    ADD CONSTRAINT opaque_login_states_identity_pair
    CHECK ((user_id IS NULL) = (tenant_id IS NULL));
