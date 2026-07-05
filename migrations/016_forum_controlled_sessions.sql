DROP INDEX IF EXISTS idx_sessions_expires;

ALTER TABLE sessions
    DROP COLUMN expires_at,
    ADD COLUMN oidc_validated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    ADD COLUMN oidc_revalidation_attempted_at TIMESTAMPTZ;

CREATE INDEX idx_sessions_last_active ON sessions(last_active_at);

ALTER TABLE oidc_login_states
    ADD COLUMN flow VARCHAR(16) NOT NULL DEFAULT 'interactive',
    ADD CONSTRAINT oidc_login_states_flow_check
        CHECK (flow IN ('interactive', 'probe', 'revalidate'));
