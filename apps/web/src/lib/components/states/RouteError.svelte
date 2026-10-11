<script lang="ts">
	import { page } from '$app/state';
	import { invalidateAll } from '$app/navigation';
	import Button from '$lib/components/ui/Button.svelte';
	import AccessDenied from './AccessDenied.svelte';
	import ErrorState from './ErrorState.svelte';

	// The shared route-error card. Used by both error boundaries at level 1
	// (there is no other heading on an error page).
	//
	// `reload` marks the root boundary: there the (app) layout itself failed
	// (e.g. the API was unreachable), so the page never loaded and a full
	// reload is the retry. In the in-shell boundary the app is already up,
	// so invalidateAll() re-runs the current route's loads.
	let { reload = false }: { reload?: boolean } = $props();

	async function retry() {
		if (reload) location.reload();
		else await invalidateAll();
	}

	const status = $derived(page.status);
	const message = $derived(page.error?.message ?? 'Something unexpected happened.');
</script>

{#if status === 403}
	<AccessDenied level={1} {message} />
{:else if status === 404}
	<ErrorState level={1} title="We can't find that page" message="The page you are looking for does not exist.">
		{#snippet actions()}
			<Button variant="ghost" href="/">Back home</Button>
		{/snippet}
	</ErrorState>
{:else if status === 502}
	<ErrorState level={1} title="We couldn't reach the server" onretry={retry} />
{:else}
	<ErrorState level={1} onretry={retry} />
{/if}
