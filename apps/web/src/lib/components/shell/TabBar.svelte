<script lang="ts">
	import Icon from '$lib/components/icons/Icon.svelte';
	import { isActive } from '$lib/nav';
	import type { NavItem } from '$lib/nav';

	// The layout passes primaryNav plus the Admin entry for admins, so the
	// column count follows the item count (5 or 6).
	let { pathname, items }: { pathname: string; items: NavItem[] } = $props();
</script>

<nav class="tabbar" aria-label="Primary" style="--tabs: {items.length}">
	{#each items as item (item.href)}
		{@const active = isActive(item.href, pathname)}
		<a
			href={item.href}
			class="tab"
			class:active={active}
			aria-current={active ? 'page' : undefined}
		>
			<Icon name={item.icon} size={20} />
			<span>{item.label}</span>
		</a>
	{/each}
</nav>

<style>
	.tabbar {
		flex: 0 0 auto;
		background: var(--minerva-surface);
		border-top: var(--minerva-border-w) solid var(--minerva-border);
		display: grid;
		grid-template-columns: repeat(var(--tabs), 1fr);
		min-height: var(--minerva-shell-tabbar-h);
		padding-bottom: env(safe-area-inset-bottom);
	}

	@media (min-width: 900px) {
		.tabbar {
			display: none;
		}
	}

	.tab {
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 3px;
		padding: 8px 2px;
		min-height: var(--minerva-shell-tabbar-h);
		color: var(--minerva-fg-muted);
		font-size: 10.5px;
		transition: color var(--minerva-motion-fast) ease;
	}

	.tab:hover {
		color: var(--minerva-fg);
	}

	.tab.active {
		color: var(--minerva-fg);
		font-weight: 500;
	}

	.tab.active :global(svg) {
		color: var(--minerva-accent);
	}
</style>
