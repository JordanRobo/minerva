//! Postgres implementation of [`SsoGroupRuleRepository`].

use application::ports::{RepositoryError, SsoGroupRuleRepository};
use diesel::prelude::*;
use domain::{SsoGroupRule, SsoGroupRuleId};

use crate::db::{PgPool, run_on_postgres};
use crate::error::map_diesel_error;
use crate::repositories::mapping::{SsoGroupRuleRow, role_to_db, sso_group_rule_from_row};
use crate::schema::sso_group_role_rules;

/// [`SsoGroupRuleRepository`] backed by Postgres through Diesel.
pub struct PostgresSsoGroupRuleRepository {
    pool: PgPool,
}

impl PostgresSsoGroupRuleRepository {
    /// Wrap a connection pool in an SSO group rule repository.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl SsoGroupRuleRepository for PostgresSsoGroupRuleRepository {
    async fn list(&self) -> Result<Vec<SsoGroupRule>, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, |conn| {
            // Group name then id: a stable order that does not depend on
            // insertion order.
            let rows: Vec<SsoGroupRuleRow> = sso_group_role_rules::table
                .order((
                    sso_group_role_rules::group_name.asc(),
                    sso_group_role_rules::id.asc(),
                ))
                .load(conn)
                .map_err(map_diesel_error)?;
            rows.into_iter().map(sso_group_rule_from_row).collect()
        })
        .await
    }

    async fn find_by_id(
        &self,
        id: SsoGroupRuleId,
    ) -> Result<Option<SsoGroupRule>, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let row: Option<SsoGroupRuleRow> = sso_group_role_rules::table
                .find(id.0)
                .first(conn)
                .optional()
                .map_err(map_diesel_error)?;
            row.map(sso_group_rule_from_row).transpose()
        })
        .await
    }

    async fn create(&self, rule: SsoGroupRule) -> Result<SsoGroupRule, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            // The unique index on group_name makes a duplicate name a
            // `Conflict` (via `map_diesel_error`).
            diesel::insert_into(sso_group_role_rules::table)
                .values((
                    sso_group_role_rules::id.eq(rule.id.0),
                    sso_group_role_rules::group_name.eq(&rule.group_name),
                    sso_group_role_rules::role.eq(role_to_db(rule.role)),
                    sso_group_role_rules::created_at.eq(&rule.created_at),
                    sso_group_role_rules::updated_at.eq(&rule.updated_at),
                ))
                .execute(conn)
                .map_err(map_diesel_error)?;
            Ok(rule)
        })
        .await
    }

    async fn update(&self, rule: SsoGroupRule) -> Result<SsoGroupRule, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let updated = diesel::update(sso_group_role_rules::table.find(rule.id.0))
                .set((
                    sso_group_role_rules::group_name.eq(&rule.group_name),
                    sso_group_role_rules::role.eq(role_to_db(rule.role)),
                    sso_group_role_rules::updated_at.eq(&rule.updated_at),
                ))
                .execute(conn)
                .map_err(map_diesel_error)?;
            if updated == 0 {
                return Err(RepositoryError::NotFound);
            }
            Ok(rule)
        })
        .await
    }

    async fn delete(&self, id: SsoGroupRuleId) -> Result<(), RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let deleted = diesel::delete(sso_group_role_rules::table.find(id.0))
                .execute(conn)
                .map_err(map_diesel_error)?;
            if deleted == 0 {
                return Err(RepositoryError::NotFound);
            }
            Ok(())
        })
        .await
    }

    async fn any_exist(&self) -> Result<bool, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, |conn| {
            let count: i64 = sso_group_role_rules::table
                .count()
                .first(conn)
                .map_err(map_diesel_error)?;
            Ok(count > 0)
        })
        .await
    }
}
