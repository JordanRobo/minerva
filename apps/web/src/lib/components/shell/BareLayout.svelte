<script lang="ts">
	import Wordmark from './Wordmark.svelte';
	import type { Snippet } from 'svelte';

	// The pre-sign-in frame (prototype's bare layout): wordmark, then one
	// centred card. No nav, no banner — there is nothing to navigate to
	// before the (app) shell exists. Also used by the root error page,
	// where the (app) layout itself failed to load.
	let { children }: { children: Snippet } = $props();
</script>

<div class="bare">
	<Wordmark big />
	<main id="main">
		<div class="card">
			{@render children()}
		</div>
	</main>
</div>

<style>
	.bare {
		min-height: 100dvh;
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 22px;
		padding: 24px;
	}

	.card {
		width: 380px;
		max-width: 100%;
		padding: 28px;
		background: var(--minerva-surface);
		border: var(--minerva-border-w) solid var(--minerva-border);
		border-radius: var(--minerva-radius);
	}
</style>
