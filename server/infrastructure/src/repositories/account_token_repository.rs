//! Postgres implementation of [`AccountTokenRepository`].

use application::ports::{
    AcceptInviteOutcome, AccountTokenRepository, RepositoryError, ResetPasswordOutcome,
};
use chrono::{DateTime, Utc};
use diesel::prelude::*;
use domain::{AccountToken, AccountTokenId, AccountTokenKind, User, UserId};

use crate::db::{PgPool, run_on_postgres};
use crate::error::map_diesel_error;
use crate::repositories::mapping::{
    AccountTokenRow, PURPOSE_INVITE, PURPOSE_PASSWORD_RESET, account_token_from_row,
    account_token_subject_to_db, role_to_db,
};
use crate::schema::{account_tokens, users};

/// [`AccountTokenRepository`] backed by Postgres through Diesel.
pub struct PostgresAccountTokenRepository {
    pool: PgPool,
}

impl PostgresAccountTokenRepository {
    /// Wrap a connection pool in an account token repository.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// Insert `token` as a fresh (unconsumed, unrevoked) row.
fn insert_token(conn: &mut PgConnection, token: &AccountToken) -> QueryResult<usize> {
    let (purpose, email, role, user_id) = account_token_subject_to_db(&token.kind);
    diesel::insert_into(account_tokens::table)
        .values((
            account_tokens::id.eq(token.id.0),
            account_tokens::purpose.eq(purpose),
            account_tokens::token_hash.eq(&token.token_hash),
            account_tokens::email.eq(email),
            account_tokens::role.eq(role),
            account_tokens::user_id.eq(user_id),
            account_tokens::created_by.eq(token.created_by.map(|id| id.0)),
            account_tokens::created_at.eq(&token.created_at),
            account_tokens::expires_at.eq(&token.expires_at),
        ))
        .execute(conn)
}

/// Revoke every live (unconsumed, unrevoked — expired or not) token for the
/// subject of `kind`: the same email for invites, the same user for resets.
fn revoke_live_for_subject(
    conn: &mut PgConnection,
    kind: &AccountTokenKind,
    now: DateTime<Utc>,
) -> QueryResult<usize> {
    let live = account_tokens::consumed_at
        .is_null()
        .and(account_tokens::revoked_at.is_null());
    match kind {
        AccountTokenKind::Invite { email, .. } => diesel::update(
            account_tokens::table.filter(
                account_tokens::purpose
                    .eq(PURPOSE_INVITE)
                    .and(account_tokens::email.eq(email))
                    .and(live),
            ),
        )
        .set(account_tokens::revoked_at.eq(Some(now)))
        .execute(conn),
        AccountTokenKind::PasswordReset { user_id } => diesel::update(
            account_tokens::table.filter(
                account_tokens::purpose
                    .eq(PURPOSE_PASSWORD_RESET)
                    .and(account_tokens::user_id.eq(user_id.0))
                    .and(live),
            ),
        )
        .set(account_tokens::revoked_at.eq(Some(now)))
        .execute(conn),
    }
}

/// Atomically claim the token: mark it consumed at `now` if it is still
/// usable (unconsumed, unrevoked, not yet expired). Returns how many rows
/// were claimed — 1 when this call got it, 0 otherwise.
fn claim_token(
    conn: &mut PgConnection,
    id: AccountTokenId,
    now: DateTime<Utc>,
) -> QueryResult<usize> {
    diesel::update(
        account_tokens::table.filter(
            account_tokens::id
                .eq(id.0)
                .and(account_tokens::consumed_at.is_null())
                .and(account_tokens::revoked_at.is_null())
                .and(account_tokens::expires_at.gt(now)),
        ),
    )
    .set(account_tokens::consumed_at.eq(Some(now)))
    .execute(conn)
}

/// Create the invited user, with the same column values as
/// `PostgresUserRepository::create`.
fn insert_user(conn: &mut PgConnection, user: &User) -> QueryResult<()> {
    diesel::insert_into(users::table)
        .values((
            users::id.eq(user.id.0),
            users::email.eq(&user.email),
            users::password_hash.eq(&user.password_hash),
            users::display_name.eq(&user.display_name),
            users::created_at.eq(&user.created_at),
            users::updated_at.eq(&user.updated_at),
            users::role.eq(role_to_db(user.role)),
            users::deactivated_at.eq(&user.deactivated_at),
        ))
        .execute(conn)?;
    Ok(())
}

/// Errors inside the accept-invite transaction, before they are mapped to
/// [`AcceptInviteOutcome`].
#[derive(Debug)]
enum AcceptTxError {
    TokenUnusable,
    EmailTaken,
    Diesel(diesel::result::Error),
}

impl From<diesel::result::Error> for AcceptTxError {
    fn from(error: diesel::result::Error) -> Self {
        AcceptTxError::Diesel(error)
    }
}

/// Errors inside the reset-password transaction, before they are mapped to
/// [`ResetPasswordOutcome`].
#[derive(Debug)]
enum ResetTxError {
    TokenUnusable,
    UserNotFound,
    Diesel(diesel::result::Error),
}

impl From<diesel::result::Error> for ResetTxError {
    fn from(error: diesel::result::Error) -> Self {
        ResetTxError::Diesel(error)
    }
}

#[async_trait::async_trait]
impl AccountTokenRepository for PostgresAccountTokenRepository {
    async fn issue(
        &self,
        token: AccountToken,
        now: DateTime<Utc>,
    ) -> Result<AccountToken, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            conn.transaction::<AccountToken, diesel::result::Error, _>(|conn| {
                // Revoke the subject's live token first so re-issuing and
                // "replace an expired invite" are one atomic step; the
                // partial unique indexes back the same rule up under
                // concurrent issuers.
                revoke_live_for_subject(conn, &token.kind, now)?;
                insert_token(conn, &token)?;
                Ok(token)
            })
            .map_err(map_diesel_error)
        })
        .await
    }

    async fn find_by_token_hash(
        &self,
        token_hash: String,
    ) -> Result<Option<AccountToken>, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let row: Option<AccountTokenRow> = account_tokens::table
                .filter(account_tokens::token_hash.eq(&token_hash))
                .first(conn)
                .optional()
                .map_err(map_diesel_error)?;
            row.map(account_token_from_row).transpose()
        })
        .await
    }

    async fn find_by_id(
        &self,
        id: AccountTokenId,
    ) -> Result<Option<AccountToken>, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let row: Option<AccountTokenRow> = account_tokens::table
                .find(id.0)
                .first(conn)
                .optional()
                .map_err(map_diesel_error)?;
            row.map(account_token_from_row).transpose()
        })
        .await
    }

    async fn list_invites(&self) -> Result<Vec<AccountToken>, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, |conn| {
            // Newest first; the id breaks created_at ties so the order is
            // deterministic.
            let rows: Vec<AccountTokenRow> = account_tokens::table
                .filter(account_tokens::purpose.eq(PURPOSE_INVITE))
                .order((account_tokens::created_at.desc(), account_tokens::id.desc()))
                .load(conn)
                .map_err(map_diesel_error)?;
            rows.into_iter().map(account_token_from_row).collect()
        })
        .await
    }

    async fn find_pending_invite_for_email(
        &self,
        email: String,
        now: DateTime<Utc>,
    ) -> Result<Option<AccountToken>, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            // Stored emails are normalized (trimmed, lowercased); match that.
            let email = email.trim().to_lowercase();
            let row: Option<AccountTokenRow> = account_tokens::table
                .filter(
                    account_tokens::purpose
                        .eq(PURPOSE_INVITE)
                        .and(account_tokens::email.eq(email))
                        .and(account_tokens::consumed_at.is_null())
                        .and(account_tokens::revoked_at.is_null())
                        .and(account_tokens::expires_at.gt(now)),
                )
                .first(conn)
                .optional()
                .map_err(map_diesel_error)?;
            row.map(account_token_from_row).transpose()
        })
        .await
    }

    async fn revoke(&self, id: AccountTokenId, now: DateTime<Utc>) -> Result<(), RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            // Only a live token flips; zero rows then means either "no such
            // token" or "already consumed or revoked".
            let updated = diesel::update(
                account_tokens::table.filter(
                    account_tokens::id
                        .eq(id.0)
                        .and(account_tokens::consumed_at.is_null())
                        .and(account_tokens::revoked_at.is_null()),
                ),
            )
            .set(account_tokens::revoked_at.eq(Some(now)))
            .execute(conn)
            .map_err(map_diesel_error)?;
            if updated == 0 {
                // Zero rows means either "no such token" or "already
                // consumed or revoked"; only the first is an error.
                let count: i64 = account_tokens::table
                    .find(id.0)
                    .count()
                    .first(conn)
                    .map_err(map_diesel_error)?;
                if count == 0 {
                    return Err(RepositoryError::NotFound);
                }
            }
            Ok(())
        })
        .await
    }

    async fn consume(
        &self,
        id: AccountTokenId,
        now: DateTime<Utc>,
    ) -> Result<bool, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let claimed = claim_token(conn, id, now).map_err(map_diesel_error)?;
            Ok(claimed == 1)
        })
        .await
    }

    async fn accept_invite(
        &self,
        id: AccountTokenId,
        user: User,
        now: DateTime<Utc>,
    ) -> Result<AcceptInviteOutcome, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let outcome = conn.transaction::<AcceptInviteOutcome, AcceptTxError, _>(|conn| {
                // Claim first: an unusable token must not create anyone.
                if claim_token(conn, id, now)? == 0 {
                    return Err(AcceptTxError::TokenUnusable);
                }
                // A taken email rolls the whole transaction back — including
                // the claim — so a failed attempt does not burn the invite.
                match insert_user(conn, &user) {
                    Ok(()) => Ok(AcceptInviteOutcome::Accepted(user)),
                    Err(diesel::result::Error::DatabaseError(
                        diesel::result::DatabaseErrorKind::UniqueViolation,
                        _,
                    )) => Err(AcceptTxError::EmailTaken),
                    Err(error) => Err(AcceptTxError::Diesel(error)),
                }
            });
            match outcome {
                Ok(outcome) => Ok(outcome),
                Err(AcceptTxError::TokenUnusable) => Ok(AcceptInviteOutcome::TokenUnusable),
                Err(AcceptTxError::EmailTaken) => Ok(AcceptInviteOutcome::EmailTaken),
                Err(AcceptTxError::Diesel(error)) => Err(map_diesel_error(error)),
            }
        })
        .await
    }

    async fn reset_password(
        &self,
        id: AccountTokenId,
        user_id: UserId,
        password_hash: String,
        now: DateTime<Utc>,
    ) -> Result<ResetPasswordOutcome, RepositoryError> {
        let pool = self.pool.clone();
        run_on_postgres(pool, move |conn| {
            let outcome = conn.transaction::<ResetPasswordOutcome, ResetTxError, _>(|conn| {
                // Claim first: an unusable token must not touch the user.
                if claim_token(conn, id, now)? == 0 {
                    return Err(ResetTxError::TokenUnusable);
                }
                // A missing user rolls back the claim too: a reset link must
                // not be burned by pointing at a gone account.
                let updated = diesel::update(users::table.find(user_id.0))
                    .set((
                        users::password_hash.eq(&password_hash),
                        users::updated_at.eq(now),
                    ))
                    .execute(conn)?;
                if updated == 0 {
                    return Err(ResetTxError::UserNotFound);
                }
                // Any other live reset link for this user stops working now.
                revoke_live_for_subject(conn, &AccountTokenKind::PasswordReset { user_id }, now)?;
                Ok(ResetPasswordOutcome::Done)
            });
            match outcome {
                Ok(outcome) => Ok(outcome),
                Err(ResetTxError::TokenUnusable) => Ok(ResetPasswordOutcome::TokenUnusable),
                Err(ResetTxError::UserNotFound) => Ok(ResetPasswordOutcome::UserNotFound),
                Err(ResetTxError::Diesel(error)) => Err(map_diesel_error(error)),
            }
        })
        .await
    }
}
