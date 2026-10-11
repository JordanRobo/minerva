import { error } from '@sveltejs/kit';
import type { Handle } from '@sveltejs/kit';
import { apiFetch } from '$lib/server/api';
import type { CurrentUser } from '$lib/server/api';

/**
 * The signed-in user for this request, or null when the browser holds no
 * valid session. Lazy and memoized per request: pages and load functions
 * that never call it make no API call.
 */
export const handle: Handle = ({ event, resolve }) => {
	let memo: CurrentUser | null | undefined;

	event.locals.currentUser = async () => {
		if (memo !== undefined) return memo;

		let response: Response;
		try {
			response = await apiFetch(event, '/api/auth/me');
		} catch {
			// Network failure or timeout: the API is down or unreachable.
			throw error(502, 'The server could not be reached');
		}

		if (response.status === 200) {
			memo = (await response.json()) as CurrentUser;
		} else if (response.status === 401) {
			memo = null;
		} else {
			// 5xx (and anything else /api/auth/me must not return): the
			// signed-out state would be a lie.
			throw error(502, 'The server could not be reached');
		}
		return memo;
	};

	return resolve(event);
};
