import { error } from '@sveltejs/kit';
import type { LayoutServerLoad } from './$types';

export const load: LayoutServerLoad = async ({ parent }) => {
	const { user } = await parent();
	if (user.role !== 'admin') {
		// Renders in the (app) +error.svelte, so the shell stays up.
		error(403, 'Only administrators can open this page.');
	}
	return { user };
};
