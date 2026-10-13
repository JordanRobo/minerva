<script lang="ts">
	import { page } from '$app/state';
	import { adminSections, isActive } from '$lib/nav';
	import type { LayoutProps } from './$types';

	let { children } = $props();
</script>

<!-- The Admin area's sub-navigation: one entry per section, above the
     page's own PageHeader. The main navigation carries a single "Admin"
     entry (nav.ts `adminEntry`); this is where the sections live. -->
<nav class="admin-tabs" aria-label="Admin">
	{#each adminSections as item (item.href)}
		{@const active = isActive(item.href, page.url.pathname)}
		<a href={item.href} class="tab" class:active={active} aria-current={active ? 'page' : undefined}>
			{item.label}
		</a>
	{/each}
</nav>

{@render children()}

<style>
	.admin-tabs {
		display: flex;
		gap: 4px;
		margin: var(--minerva-space-2) 0 var(--minerva-space-3);
	}

	.tab {
		padding: 8px 14px;
		border-radius: 999px;
		font-size: 14.5px;
		font-weight: 500;
		color: var(--minerva-fg-muted);
		transition: background var(--minerva-motion-fast) ease, color var(--minerva-motion-fast) ease;
	}

	.tab:hover {
		background: color-mix(in oklch, var(--minerva-fg) 6%, transparent);
		color: var(--minerva-fg);
	}

	.tab.active {
		background: color-mix(in oklch, var(--minerva-accent) 12%, transparent);
		color: var(--minerva-fg);
	}
</style>
