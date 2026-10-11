<script lang="ts">
	import Icon from '$lib/components/icons/Icon.svelte';
	import type { Snippet } from 'svelte';

	// One neutral notice style; pages render it above their PageHeader.
	// Dismissal is local state only — no persistence, no global store.
	let { dismissible = false, children }: { dismissible?: boolean; children: Snippet } = $props();

	let dismissed = $state(false);
</script>

{#if !dismissed}
	<div class="banner" role="status">
		<Icon name="info" size={16} />
		<p>{@render children()}</p>
		{#if dismissible}
			<button class="close" type="button" aria-label="Dismiss notice" onclick={() => (dismissed = true)}>
				<Icon name="close" size={14} />
			</button>
		{/if}
	</div>
{/if}

<style>
	.banner {
		display: flex;
		align-items: center;
		gap: 10px;
		margin: 0 0 var(--minerva-space-4);
		padding: 10px 10px 10px 14px;
		background: var(--minerva-surface);
		border: var(--minerva-border-w) solid var(--minerva-border);
		border-radius: var(--minerva-radius);
		font-size: 14px;
		color: var(--minerva-fg);
	}

	.banner :global(.ic) {
		color: var(--minerva-fg-muted);
	}

	.banner p {
		flex: 1;
		min-width: 0;
		margin: 0;
	}

	.close {
		flex: 0 0 auto;
		width: 30px;
		height: 30px;
		display: grid;
		place-items: center;
		border: 0;
		border-radius: 8px;
		background: transparent;
		color: var(--minerva-fg-muted);
		cursor: pointer;
	}

	.close:hover {
		background: color-mix(in oklch, var(--minerva-fg) 6%, transparent);
		color: var(--minerva-fg);
	}
</style>
