<script lang="ts">
	import { Tooltip } from 'bits-ui';
	import Icon from '$lib/components/icons/Icon.svelte';
	import type { NavItem } from '$lib/nav';

	let { item, active, collapsed }: { item: NavItem; active: boolean; collapsed: boolean } =
		$props();
</script>

{#if collapsed}
	<!-- Collapsed: the label is visually hidden but present (the link keeps its
	     accessible name); the tooltip replaces the old title=, which only worked
	     on hover and never on keyboard focus. -->
	<Tooltip.Root>
		<Tooltip.Trigger>
			{#snippet child({ props })}
				<a
					href={item.href}
					class="navlink collapsed"
					class:active={active}
					aria-current={active ? 'page' : undefined}
					{...props}
				>
					<Icon name={item.icon} />
					<span class="navlabel vh">{item.label}</span>
				</a>
			{/snippet}
		</Tooltip.Trigger>
		<Tooltip.Content side="right" sideOffset={6}>
			<span class="minerva-tooltip" role="tooltip">{item.label}</span>
		</Tooltip.Content>
	</Tooltip.Root>
{:else}
	<a
		href={item.href}
		class="navlink"
		class:active={active}
		aria-current={active ? 'page' : undefined}
	>
		<Icon name={item.icon} />
		<span class="navlabel">{item.label}</span>
	</a>
{/if}

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
