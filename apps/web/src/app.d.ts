// See https://svelte.dev/docs/kit/types#app.d.ts
// for information about these interfaces
import type { CurrentUser } from '$lib/server/api';

declare global {
	namespace App {
		// interface Error {}
		interface Locals {
			/**
			 * The signed-in user for this request, or null when the
			 * browser holds no valid session. Set by hooks.server.ts;
			 * calling it makes the API call, so only call it when needed.
			 */
			currentUser: () => Promise<CurrentUser | null>;
		}
		// interface PageData {}
		// interface PageState {}
		// interface Platform {}
	}
}

export {};
