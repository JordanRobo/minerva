-- Roadmap 2.7 (step 2): SSO group-to-role mapping rules (D15).
--
-- One row maps one IdP group name to a role. While at least one rule exists,
-- every SSO sign-in recomputes the user's role from the groups claim; with no
-- rules, roles are never touched. Group names are matched exactly and
-- case-sensitively after trimming surrounding whitespace, so they are stored
-- as entered (no normalisation) and one rule per name is enforced by a unique
-- index on the raw column. The CHECK keeps a rule from matching nothing.
CREATE TABLE sso_group_role_rules (
    id UUID PRIMARY KEY,
    group_name TEXT NOT NULL CHECK (btrim(group_name) <> ''),
    role TEXT NOT NULL CHECK (role IN ('admin', 'staff', 'read_only')),
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);

-- One rule per group name (D15).
CREATE UNIQUE INDEX idx_sso_group_role_rules_group_name
    ON sso_group_role_rules (group_name);
