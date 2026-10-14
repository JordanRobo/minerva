//! The people picker (roadmap 3.11, D17): the person summary that task, goal
//! and milestone responses embed for their assignee or owner, the batch
//! resolver that fills those summaries with one user query per response or
//! page, and the `GET /api/people` endpoint that feeds the pickers in the UI.
//!
//! The summary carries only what a picker needs to render a name — never the
//! email (D17). A deactivated person still renders, flagged by `active`.

use actix_web::{HttpResponse, web};
use application::ports::{RepositoryError, UserRepository};
use domain::{UserId, UserSummary};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::access::ViewAccess;
use crate::error::{ApiError, repo_error_response};

/// JSON shape of a person in task responses and the people picker (D17):
/// just enough to render a name. The email is never part of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct PersonSummary {
    pub id: Uuid,
    pub display_name: String,
    /// Whether the account may sign in (that is, has not been deactivated).
    pub active: bool,
}

impl From<&UserSummary> for PersonSummary {
    fn from(summary: &UserSummary) -> Self {
        Self {
            id: summary.id.0,
            display_name: summary.display_name.clone(),
            active: summary.active,
        }
    }
}

/// Body for `PUT /api/goals/{id}/owner` and `PUT /api/milestones/{id}/owner`:
/// the person to name as owner. One shared type because both routes take the
/// same body (D17).
#[derive(Deserialize, ToSchema)]
pub struct OwnerRequest {
    /// The user to make the owner; must be an existing active account.
    pub owner_id: Uuid,
}

/// Resolve person summaries for a batch of user references (task assignees,
/// goal and milestone owners) with ONE query (D17): a page of fifty rows
/// shares a single `summaries_by_ids` call instead of one per row. Duplicate
/// ids collapse to one lookup; ids that do not resolve (unknown users) are
/// simply absent from the result.
pub async fn resolve_summaries(
    users: &dyn UserRepository,
    assignee_ids: impl IntoIterator<Item = Option<UserId>>,
) -> Result<HashMap<Uuid, PersonSummary>, RepositoryError> {
    let wanted: BTreeSet<Uuid> = assignee_ids.into_iter().flatten().map(|id| id.0).collect();
    if wanted.is_empty() {
        return Ok(HashMap::new());
    }
    let ids: Vec<UserId> = wanted.into_iter().map(UserId).collect();
    let summaries = users.summaries_by_ids(&ids).await?;
    Ok(summaries
        .into_iter()
        .map(|summary| (summary.id.0, PersonSummary::from(&summary)))
        .collect())
}

/// List People
///
/// List every active user as a person summary, ordered by display name then id — the data for assignee pickers. Deactivated accounts are excluded; the email is never included. Any signed-in user may read. A plain array rather than a page envelope: the list is small and bounded (a school's staff), like users, invites and SSO group rules.
#[utoipa::path(
    get,
    path = "/api/people",
    tags = ["people"],
    security(("session_cookie" = [])),
    responses(
        (
            status = 200,
            description = "Every active user as a person summary",
            body = Vec<PersonSummary>
        ),
        (status = 401, description = "Missing or invalid session", body = ApiError)
    )
)]
pub async fn list_people(
    users: web::Data<dyn UserRepository>,
    _access: ViewAccess,
) -> Result<HttpResponse, ApiError> {
    let summaries = users
        .list_active_summaries()
        .await
        .map_err(repo_error_response)?;
    Ok(HttpResponse::Ok().json(
        summaries
            .iter()
            .map(PersonSummary::from)
            .collect::<Vec<_>>(),
    ))
}
