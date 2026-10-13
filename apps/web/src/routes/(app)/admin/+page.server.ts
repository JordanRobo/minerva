import { redirect } from '@sveltejs/kit';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async ({ parent }) => {
	// Runs the admin guard first (403 for non-admins), then lands on People.
	await parent();
	redirect(307, '/admin/people');
};
