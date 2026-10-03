-- Roadmap 2.4 (step 5): an admin can deactivate an account. NULL means the
-- account is active; the timestamp records when it was deactivated.
ALTER TABLE users ADD COLUMN deactivated_at TIMESTAMPTZ;
