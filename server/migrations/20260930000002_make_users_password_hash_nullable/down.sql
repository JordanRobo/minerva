-- Fails if any user row has a NULL password_hash (a passwordless account),
-- because Postgres cannot restore NOT NULL while NULLs exist.
ALTER TABLE users ALTER COLUMN password_hash SET NOT NULL;
