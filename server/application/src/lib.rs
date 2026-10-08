//! Minerva application layer.
//!
//! Use cases and orchestration over the domain. Depends on `domain` plus
//! `sha2`, used for one-way subject hashing in [`rate_limit`] (the same
//! hashing the session tokens use) — no other non-domain dependency.

pub mod account_links;
pub mod auth;
pub mod authz;
pub mod bootstrap;
pub mod goal_milestone_links;
pub mod oidc_login;
pub mod ports;
pub mod rate_limit;
pub mod sso_roles;
pub mod sso_rules;
pub mod status_override;
pub mod task_relations;
pub mod task_status;
pub mod user_admin;
