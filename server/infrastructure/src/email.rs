//! An [`AccountEmailSender`] with no delivery channel behind it. Roadmap 7.3
//! supplies the SMTP implementation; until then every send fails and the use
//! cases return their links to the caller instead of emailing them.

use application::ports::{AccountEmailSender, EmailSendError};
use chrono::{DateTime, Utc};
use domain::Role;

/// A sender that never delivers: `is_configured` is false and every send
/// fails with the same error.
pub struct NoEmailSender;

#[async_trait::async_trait]
impl AccountEmailSender for NoEmailSender {
    fn is_configured(&self) -> bool {
        false
    }

    async fn send_invite(
        &self,
        _to_email: &str,
        _link: &str,
        _role: Role,
        _expires_at: DateTime<Utc>,
    ) -> Result<(), EmailSendError> {
        Err(EmailSendError(
            "email delivery is not configured".to_owned(),
        ))
    }

    async fn send_password_reset(
        &self,
        _to_email: &str,
        _link: &str,
        _expires_at: DateTime<Utc>,
    ) -> Result<(), EmailSendError> {
        Err(EmailSendError(
            "email delivery is not configured".to_owned(),
        ))
    }
}
