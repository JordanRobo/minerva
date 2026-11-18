-- Roadmap 3.12 (D20): flat, plain-text comments on a task. A comment belongs
-- to one task and is deleted with it; the author is a nullable user reference
-- so deleting a user leaves their comments in place as unattributed
-- (ON DELETE SET NULL, like D17).
CREATE TABLE task_comments (
    id UUID PRIMARY KEY,
    task_id UUID NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    author_id UUID REFERENCES users(id) ON DELETE SET NULL,
    -- Mirrors the domain rule (normalize_comment_body): non-blank after
    -- trimming whitespace, including newlines, and at most 4000 characters.
    -- The 'g' flag is required: without it only the first match (the leading
    -- run) would be trimmed and trailing whitespace would count toward the
    -- length.
    body TEXT NOT NULL CHECK (
        char_length(regexp_replace(body, '^\s+|\s+$', '', 'g')) BETWEEN 1 AND 4000
    ),
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);

-- A task's thread reads oldest first (created_at, then id), which is the
-- list order (roadmap 3.12).
CREATE INDEX idx_task_comments_task_id ON task_comments (task_id, created_at, id);
