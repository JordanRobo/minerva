import { error } from '@sveltejs/kit';
import { dev } from '$app/environment';
import type { PageServerLoad } from './$types';

// Dev-only component gallery: absent from production builds. The parent
// (auth guard) runs first, so signed-out visitors get the usual redirect.
export const load: PageServerLoad = async ({ parent }) => {
	const { user } = await parent();
	if (!dev) error(404, 'Not found.');
	return { user };
};
