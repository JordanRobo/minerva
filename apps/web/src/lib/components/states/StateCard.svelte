<script lang="ts">
	import Icon from '$lib/components/icons/Icon.svelte';
	import type { IconName } from '$lib/components/icons/Icon.svelte';
	import type { Snippet } from 'svelte';

	// Shared frame for the page states (prototype's .state-center): centred
	// surface card, 48px circular icon, heading, message, actions.
	let {
		icon,
		level = 2,
		title,
		message,
		hint,
		actions
	}: {
		icon: IconName;
		level?: 1 | 2 | 3;
		title: string;
		message: string;
		hint?: string;
		actions?: Snippet;
	} = $props();

	const heading: 'h1' | 'h2' | 'h3' = $derived(level === 1 ? 'h1' : level === 2 ? 'h2' : 'h3');
</script>

<div class="state">
	<div class="icon">
		<Icon name={icon} size={22} />
	</div>
	<svelte:element this={heading} class="title">{title}</svelte:element>
	<p>{message}</p>
	{#if hint}
		<p class="hint">{hint}</p>
	{/if}
	{#if actions}
		<div class="actions">{@render actions()}</div>
	{/if}
</div>

<style>
	.state {
		padding: 56px 24px;
		background: var(--minerva-surface);
		border: var(--minerva-border-w) solid var(--minerva-border);
		border-radius: var(--minerva-radius);
		text-align: center;
	}

	.icon {
		width: 48px;
		height: 48px;
		margin: 0 auto 16px;
		display: grid;
		place-items: center;
		border: var(--minerva-border-w) solid var(--minerva-border);
		border-radius: 50%;
		background: color-mix(in oklch, var(--minerva-fg) 4%, transparent);
		color: var(--minerva-fg-muted);
	}

	.title {
		margin: 0;
		font-size: 21px;
		letter-spacing: -0.01em;
	}

	.state > p {
		margin: 8px auto 0;
		max-width: 44ch;
		font-size: 15px;
		color: var(--minerva-fg-muted);
	}

	.hint {
		font-size: 14px;
	}

	.actions {
		display: flex;
		align-items: center;
		justify-content: center;
		gap: 10px;
		flex-wrap: wrap;
		margin-top: 20px;
	}
</style>
