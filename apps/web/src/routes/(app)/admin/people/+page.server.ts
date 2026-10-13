import type { PageServerLoad } from './$types';

// The guard (and the signed-in admin) come from the layouts; the list itself
// is fetched in the browser — it is an interactive screen whose rows change
// with every action, and the actions are same-origin browser fetches anyway.
export const load: PageServerLoad = async ({ parent }) => {
	const { user } = await parent();
	return { user };
};
