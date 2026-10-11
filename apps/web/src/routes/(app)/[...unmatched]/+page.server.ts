import { error } from '@sveltejs/kit';
import type { PageServerLoad } from './$types';

// Anything that reaches here matched no real route. 404 from inside the
// (app) group so the in-shell error boundary renders it — a root-level 404
// would fall through to the bare pre-sign-in frame instead.
export const load: PageServerLoad = async ({ parent }) => {
	await parent();
	error(404, 'The page you are looking for does not exist.');
};
