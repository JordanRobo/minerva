-- Roadmap 2.7 (D15): drop the SSO flags again.
ALTER TABLE users DROP COLUMN role_managed_by_sso;
ALTER TABLE users DROP COLUMN sso_role_exempt;
