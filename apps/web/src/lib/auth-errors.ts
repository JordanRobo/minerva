// Plain-language copy for the error codes the SSO redirect flow bounces back
// on as /login?error=<code> (server/interface/src/redirect.rs). The complete
// set of codes today:
//
//   login_failed            any credential-flow or IdP failure we can't name
//   oidc_provider_error     the IdP answered with an error
//   oidc_login_failed       the IdP rejected the credentials
//   oidc_unavailable        the IdP could not be reached
//   oidc_email_missing      the IdP profile had no email
//   oidc_email_not_verified the IdP email was unverified
//   oidc_signup_disabled    no local account and sign-up is off
//   deactivated             the account is turned off
//
// Only the last two get their own wording; everything else (including codes
// added later) falls through to the generic message. The raw code is shown
// verbatim as small "Reference: …" text — rendered as text, never HTML.

export function ssoErrorMessage(code: string): string {
	switch (code) {
		case 'oidc_signup_disabled':
			return "Your account hasn't been set up yet. Ask your school's administrator to invite you.";
		case 'deactivated':
			return 'This account has been turned off. Contact your administrator.';
		default:
			return "We couldn't sign you in. Please try again.";
	}
}

/**
 * Plain-language text for a failed postJson() call, shared by the login,
 * accept-invite and reset-password forms. 400 bodies already carry
 * plain-language server messages ("the password must be at least 8
 * characters"); everything else is generalised so no technical detail leaks
 * and no error ever names which part of a check failed.
 */
export function postErrorMessage(res: { code: string; status: number; message: string; retryAfter?: number }): string {
	if (res.code === 'rate_limited') {
		const minutes = Math.max(1, Math.ceil((res.retryAfter ?? 60) / 60));
		return `Too many attempts. Please wait ${minutes} minute${minutes === 1 ? '' : 's'} and try again.`;
	}
	if (res.code === 'network') {
		return "We couldn't reach the server. Check your connection and try again.";
	}
	if (res.status === 400 && res.message) return res.message;
	return 'Something went wrong. Please try again.';
}
