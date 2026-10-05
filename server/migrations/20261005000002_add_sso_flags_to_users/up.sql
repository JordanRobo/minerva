-- Roadmap 2.7 (D15): two flags on the user for SSO role management.
-- `role_managed_by_sso` is set by the sign-in recomputation (a later step)
-- and, while any group rule exists, locks the role against hand edits;
-- `sso_role_exempt` marks the 2.5 bootstrap admin, which is never recomputed.
ALTER TABLE users ADD COLUMN role_managed_by_sso BOOLEAN NOT NULL DEFAULT false;
ALTER TABLE users ADD COLUMN sso_role_exempt BOOLEAN NOT NULL DEFAULT false;
