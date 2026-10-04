-- Roadmap 2.6 (step 1): single-use links for invites and password resets.
--
-- One table serves both purposes so the two flows share one mechanism:
-- an invite names an email and a role (no user exists yet), a password
-- reset names the user whose password changes. Only the hash of the token
-- is stored — the raw value lives in the link and is never kept here.
CREATE TABLE account_tokens (
    id UUID PRIMARY KEY,
    purpose TEXT NOT NULL CHECK (purpose IN ('invite', 'password_reset')),
    token_hash TEXT UNIQUE NOT NULL,
    email TEXT,
    role TEXT CHECK (role IN ('admin', 'staff', 'read_only')),
    user_id UUID REFERENCES users(id) ON DELETE CASCADE,
    created_by UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    consumed_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ
);

-- Each purpose fills exactly its own subject columns: an invite has an
-- email and a role and no user (there is none yet), a reset has a user and
-- no email or role. Enforced here so no row can ever mean nothing; the
-- application stores emails already normalized (trimmed, lowercased).
ALTER TABLE account_tokens ADD CONSTRAINT account_tokens_kind CHECK (
    (purpose = 'invite' AND email IS NOT NULL AND role IS NOT NULL AND user_id IS NULL)
    OR
    (purpose = 'password_reset' AND user_id IS NOT NULL AND email IS NULL AND role IS NULL)
);

-- At most one live (unconsumed, unrevoked) token per subject. Re-issuing an
-- invite or reset therefore replaces the old token: the application revokes
-- the live one and inserts the new in one transaction, and these indexes
-- make that rule hold even under concurrent issuers. Expired tokens are
-- revoked on re-issue too — they still occupy the subject until then.
CREATE UNIQUE INDEX idx_account_tokens_live_invite_email
    ON account_tokens (email)
    WHERE purpose = 'invite' AND consumed_at IS NULL AND revoked_at IS NULL;

CREATE UNIQUE INDEX idx_account_tokens_live_reset_user_id
    ON account_tokens (user_id)
    WHERE purpose = 'password_reset' AND consumed_at IS NULL AND revoked_at IS NULL;

-- Resets and user lookups filter by user_id; the partial index above only
-- covers live reset tokens.
CREATE INDEX idx_account_tokens_user_id ON account_tokens (user_id);
