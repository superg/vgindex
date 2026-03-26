CREATE TABLE oauth_authorization_codes (
    code VARCHAR(128) PRIMARY KEY,
    client_id VARCHAR(128) NOT NULL,
    user_id INT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    redirect_uri TEXT NOT NULL,
    scope VARCHAR(255) NOT NULL DEFAULT 'openid',
    nonce VARCHAR(255),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE oauth_access_tokens (
    token VARCHAR(128) PRIMARY KEY,
    user_id INT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    client_id VARCHAR(128) NOT NULL,
    scope VARCHAR(255) NOT NULL DEFAULT 'openid',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ NOT NULL
);
