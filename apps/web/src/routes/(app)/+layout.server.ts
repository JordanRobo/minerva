import { redirect } from '@sveltejs/kit';
import type { LayoutServerLoad } from './$types';

export const load: LayoutServerLoad = async ({ url, locals }) => {
	// Reading `url` keeps this load re-running on every client-side
	// navigation, so a signed-out tab is bounced on the next click, not
	// only on a full reload.
	const user = await locals.currentUser();
	if (!user) {
		redirect(303, `/login?next=${encodeURIComponent(url.pathname + url.search)}`);
	}
	return { user };
};
