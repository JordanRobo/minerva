<script lang="ts">
	import Icon from '$lib/components/icons/Icon.svelte';
	import type { NavItem } from '$lib/nav';

	let { item, active, collapsed }: { item: NavItem; active: boolean; collapsed: boolean } =
		$props();
</script>

<a
	href={item.href}
	class="navlink"
	class:active={active}
	class:collapsed={collapsed}
	aria-current={active ? 'page' : undefined}
	title={collapsed ? item.label : undefined}
>
	<Icon name={item.icon} />
	<!-- Visually hidden when collapsed, never display:none: the link keeps
	     its accessible name either way. -->
	<span class="navlabel" class:vh={collapsed}>{item.label}</span>
</a>

<style>
	.navlink {
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 9px 12px;
		border-radius: 8px;
		font-size: 14.5px;
		font-weight: 500;
		color: var(--minerva-fg);
		transition: background var(--minerva-motion-fast) ease;
	}

	.navlink :global(svg) {
		color: var(--minerva-fg-muted);
	}

	.navlink:hover {
		background: color-mix(in oklch, var(--minerva-fg) 6%, transparent);
	}

	.navlink:hover :global(svg) {
		color: var(--minerva-fg);
	}

	.navlink.active {
		background: color-mix(in oklch, var(--minerva-accent) 12%, transparent);
	}

	.navlink.active :global(svg) {
		color: var(--minerva-accent);
	}

	.navlink.collapsed {
		justify-content: center;
		padding: 11px 0;
	}
</style>
