import { env } from '$env/dynamic/private';
import type { RequestEvent } from '@sveltejs/kit';

/**
 * The role exactly as the API serializes it — snake_case, so note the
 * underscore in `read_only` (server `domain::Role`).
 */
export type Role = 'admin' | 'staff' | 'read_only';

/**
 * The signed-in user, mirroring the server's `UserResponse`
 * (server/interface/src/auth.rs). Hand-written: the OpenAPI client is not
 * generated (roadmap constraint).
 */
export interface CurrentUser {
	id: string;
	email: string;
	display_name: string;
	role: Role;
	created_at: string;
	updated_at: string;
}

/**
 * One entry of `GET /api/users` and the user-admin mutation responses
 * (server/interface/src/users.rs, `UserAdminResponse`). The password hash is
 * never part of the wire format. `role_managed_by_sso` is the EFFECTIVE D15
 * lock — the user's flag AND any group rule exists — not the raw flag.
 */
export interface AdminUser {
	id: string;
	email: string;
	display_name: string;
	role: Role;
	active: boolean;
	deactivated_at: string | null;
	role_managed_by_sso: boolean;
	created_at: string;
	updated_at: string;
}

/** `POST /api/users/{id}/password-reset` (server/interface/src/users.rs). */
export interface PasswordResetLink {
	link: string;
	expires_at: string;
	emailed: boolean;
}

/** One entry of `GET /api/invites` (server/interface/src/invites.rs). The raw
 *  token is never part of the wire format — it was shown exactly once, when
 *  the invite was created or re-issued. */
export interface Invite {
	id: string;
	email: string;
	role: Role;
	status: 'pending' | 'accepted' | 'revoked' | 'expired';
	created_by: string | null;
	created_at: string;
	expires_at: string;
	consumed_at: string | null;
	revoked_at: string | null;
}

/** `POST /api/invites` and `POST /api/invites/{id}/reissue` (201). The link is
 *  shown only in this response; a re-issue is a fresh token row, so the invite
 *  id changes. */
export interface InviteCreated {
	invite: Invite;
	link: string;
	emailed: boolean;
}

/** One entry of `GET /api/sso/group-rules` (server/interface/src/sso_rules.rs). */
export interface SsoGroupRule {
	id: string;
	group_name: string;
	role: Role;
	created_at: string;
	updated_at: string;
}

/**
 * One entry of `GET /api/auth/providers` (server/interface/src/auth.rs):
 * `credentials` providers (the email/password form) versus `redirect`
 * providers (one "Sign in with …" button each, linking to `login_url`).
 */
export interface AuthProvider {
	id: string;
	display_name: string;
	kind: 'credentials' | 'redirect';
	login_url: string;
}

/**
 * `POST /api/auth/tokens/inspect` (server/interface/src/account_links.rs).
 * `role` is set for invites, `null` for password resets.
 */
export interface TokenLinkInfo {
	purpose: 'invite' | 'password_reset';
	email: string;
	role: Role | null;
	expires_at: string;
}

// Dev-mode fallback only (bun run dev with the compose stack or the API run
// locally): host 8080 is occupied on the maintainer's machine, so the dev
// compose stack publishes the API on 3010.
const DEV_API_URL = 'http://localhost:3010';

const API_TIMEOUT_MS = 5_000;

function apiBase(): string {
	if (env.API_INTERNAL_URL) return env.API_INTERNAL_URL;
	if (import.meta.env.DEV) return DEV_API_URL;
	throw new Error(
		'API_INTERNAL_URL is not set. It must point at the Minerva API over the internal ' +
			'network (e.g. http://server:8080 in the dev compose stack) — see README, "Frontend only".'
	);
}

/**
 * Call the API from server-side code (hooks, load functions) against the
 * internal service URL — never the public origin. The incoming request's
 * `Cookie` header is forwarded explicitly, because SvelteKit's automatic
 * cookie forwarding only applies to same-origin fetches. The returned
 * response's `Set-Cookie` must never be copied back to the browser:
 * sign-in and sign-out are the browser's job, calling `/api/*` through the
 * proxy.
 */
export function apiFetch(
	event: RequestEvent,
	path: string,
	init: RequestInit = {}
): Promise<Response> {
	const headers = new Headers(init.headers);
	headers.set('Accept', 'application/json');
	const cookie = event.request.headers.get('cookie');
	if (cookie) headers.set('Cookie', cookie);

	return fetch(`${apiBase()}${path}`, {
		...init,
		headers,
		signal: AbortSignal.timeout(API_TIMEOUT_MS)
	});
}
