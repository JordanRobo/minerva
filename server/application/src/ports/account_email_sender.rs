//! Port for sending the account-link emails (invites and password resets).

use chrono::{DateTime, Utc};
use domain::Role;

/// A failure to send an email.
#[derive(Debug)]
pub struct EmailSendError(pub String);

impl std::fmt::Display for EmailSendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for EmailSendError {}

/// Sends the account-link emails.
///
/// Delivery is best-effort from the use cases' point of view: when a send
/// fails, the link is still returned to the caller so it can be shared by
/// hand. Roadmap 7.3 supplies the SMTP implementation behind this port; until
/// then no delivery channel is configured and every send fails.
#[async_trait::async_trait]
pub trait AccountEmailSender: Send + Sync {
    /// Whether an actual delivery channel is configured.
    fn is_configured(&self) -> bool;

    /// Invite `to_email` to create an account with `role`. The link and the
    /// expiry go into the message body, so they are passed rather than read
    /// back from storage.
    async fn send_invite(
        &self,
        to_email: &str,
        link: &str,
        role: Role,
        expires_at: DateTime<Utc>,
    ) -> Result<(), EmailSendError>;

    /// Let the holder of the account behind `to_email` set a new password.
    async fn send_password_reset(
        &self,
        to_email: &str,
        link: &str,
        expires_at: DateTime<Utc>,
    ) -> Result<(), EmailSendError>;
}
