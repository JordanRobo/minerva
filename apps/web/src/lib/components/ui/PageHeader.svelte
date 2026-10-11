<script lang="ts">
	import type { Snippet } from 'svelte';

	// Role-unaware on purpose: pages pass `actions` only when
	// can(role, ...) allows, so nothing renders for a role that lacks it.
	let {
		title,
		description,
		documentTitle,
		actions,
		secondary
	}: {
		title: string;
		description?: string;
		documentTitle?: string;
		actions?: Snippet; // primary slot
		secondary?: Snippet;
	} = $props();
</script>

<svelte:head>
	<title>{documentTitle ?? `${title} · Minerva`}</title>
</svelte:head>

<header class="page-header">
	<div class="title">
		<h1>{title}</h1>
		{#if description}
			<p class="desc">{description}</p>
		{/if}
	</div>
	{#if actions || secondary}
		<div class="actions">
			{@render secondary?.()}
			{@render actions?.()}
		</div>
	{/if}
</header>

<style>
	.page-header {
		display: flex;
		align-items: flex-end;
		justify-content: space-between;
		gap: 16px;
		flex-wrap: wrap;
		padding: 22px 0 18px;
	}

	h1 {
		margin: 0;
		font-size: 21px;
		line-height: 1.15;
		letter-spacing: -0.01em;
	}

	.desc {
		margin: 2px 0 0;
		font-size: 13.5px;
		color: var(--minerva-fg-muted);
	}

	/* Wraps below the title on narrow screens via the parent's flex-wrap. */
	.actions {
		display: flex;
		align-items: center;
		gap: 10px;
		flex-wrap: wrap;
	}

	@media (min-width: 900px) {
		h1 {
			font-size: 27px;
		}

		.desc {
			margin-top: 4px;
			font-size: 14.5px;
		}
	}
</style>
