import type { PageServerLoad } from './$types';

export const load: PageServerLoad = ({ url, setHeaders }) => {
	// The token sits in this page's URL. Keep it out of the Referer header
	// (leaks to any cross-origin request the page could make) and out of
	// caches. It travels to the API in the request body only.
	setHeaders({
		'referrer-policy': 'no-referrer',
		'cache-control': 'no-store'
	});
	return { token: url.searchParams.get('token') ?? '' };
};
