import adapter from '@sveltejs/adapter-node';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig, loadEnv } from 'vite';

// The web app must never define routes under /api or /api-docs: in dev the
// proxy below forwards those paths to the API (see README, "Frontend only").
export default defineConfig(({ mode }) => {
	const { API_INTERNAL_URL } = loadEnv(mode, '.', 'API_');
	return {
		plugins: [
			sveltekit({
				compilerOptions: {
					// Force runes mode for the project, except for libraries. Can be removed in svelte 6.
					runes: ({ filename }) =>
						filename.split(/[/\\]/).includes('node_modules') ? undefined : true
				},

				// Serves the app with `bun build/index.js` (see deploy/docker/web.Dockerfile).
				adapter: adapter()
			})
		],
		server: {
			allowedHosts: true,
			proxy: {
				// Regex key: only /api/... and /api-docs/... are forwarded,
				// never a web route that merely starts with "/api" (e.g.
				// /apiary). Host header and cookies pass through untouched.
				// Default: where the dev compose stack publishes the API
				// (host 8080 is occupied on the maintainer's machine).
				'^/api(-docs)?/': API_INTERNAL_URL || 'http://localhost:3010'
			}
		}
	};
});
