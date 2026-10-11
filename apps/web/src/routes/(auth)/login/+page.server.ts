import { redirect } from '@sveltejs/kit';
import { apiFetch } from '$lib/server/api';
import type { AuthProvider } from '$lib/server/api';
import { safeNext } from '$lib/safe-redirect';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	if (await event.locals.currentUser()) {
		redirect(303, safeNext(event.url.searchParams.get('next')));
	}

	// The API decides which sign-in options exist. An unreachable API throws
	// into the 502 error page ("couldn't reach the server"); a reachable API
	// with no providers renders the "not set up yet" copy below.
	const res = await apiFetch(event, '/api/auth/providers');
	const providers: AuthProvider[] = res.ok
		? ((await res.json()) as { providers: AuthProvider[] }).providers
		: [];

	const notice = event.url.searchParams.get('notice');
	return {
		providers,
		next: safeNext(event.url.searchParams.get('next')),
		// SSO failures bounce back here as ?error=<code> (redirect.rs).
		error: event.url.searchParams.get('error'),
		// Only known notice values are surfaced; anything else is ignored.
		notice: notice === 'password_reset' ? 'password_reset' : null
	};
};
