-- Roadmap 2.4 (step 1): every user has a role.
--
-- The column is added with a temporary DEFAULT so Postgres can backfill the
-- existing rows. The single oldest user is then promoted to 'admin' so an
-- already-populated dev database keeps an administrator until roadmap 2.5
-- introduces account management; on a fresh database the UPDATE matches
-- nothing. Finally the default is dropped so every insert from now on must
-- state its role explicitly — the application always supplies one, like for
-- every other column in this schema.
ALTER TABLE users
    ADD COLUMN role TEXT NOT NULL DEFAULT 'read_only'
        CHECK (role IN ('admin', 'staff', 'read_only'));

UPDATE users
SET role = 'admin'
WHERE id = (SELECT id FROM users ORDER BY created_at ASC LIMIT 1);

ALTER TABLE users ALTER COLUMN role DROP DEFAULT;
