<script lang="ts">
	import { Popover, Tooltip } from 'bits-ui';
	import Icon from '$lib/components/icons/Icon.svelte';
	import { adminEntry, isActive, primaryNav, roleLabel } from '$lib/nav';
	import type { CurrentUser } from '$lib/server/api';
	import Wordmark from './Wordmark.svelte';
	import NavLink from './NavLink.svelte';

	let {
		user,
		initials,
		isAdmin,
		collapsed,
		pathname,
		signOutError,
		onToggleCollapse,
		onSignOut
	}: {
		user: CurrentUser;
		initials: string;
		isAdmin: boolean;
		collapsed: boolean;
		pathname: string;
		signOutError: string | null;
		onToggleCollapse: () => void;
		onSignOut: () => void;
	} = $props();

	const name = $derived(user.display_name || user.email);

	// Desktop user menu: a Bits UI Popover (D18), deliberately not a
	// DropdownMenu — the panel holds one action, so it keeps disclosure
	// semantics (aria-expanded on the trigger) rather than role="menu". The
	// primitive brings Escape/outside-click closing and focus return to the
	// trigger; trapFocus is off because the old disclosure never trapped.
</script>

<aside class="sidebar" class:collapsed={collapsed}>
	<div class="brand-row">
		<Wordmark collapsed={collapsed} />
		<Tooltip.Root>
			<Tooltip.Trigger>
				{#snippet child({ props })}
					<!-- {...props} before onclick: the trigger's own click handler would
					     otherwise clobber the toggle (a pointer press already dismisses the
					     tooltip via pointerdown). -->
					<button
						class="collapse-btn"
						class:collapsed={collapsed}
						type="button"
						aria-label={collapsed ? 'Expand sidebar' : 'Collapse sidebar'}
						aria-expanded={!collapsed}
						{...props}
						onclick={onToggleCollapse}
					>
						<Icon name="chevron" size={16} />
					</button>
				{/snippet}
			</Tooltip.Trigger>
			<Tooltip.Content side="right" sideOffset={6}>
				<span class="minerva-tooltip" role="tooltip">{collapsed ? 'Expand' : 'Collapse'}</span>
			</Tooltip.Content>
		</Tooltip.Root>
	</div>

	<nav class="nav" aria-label="Primary">
		{#each primaryNav as item (item.href)}
			<NavLink item={item} active={isActive(item.href, pathname)} collapsed={collapsed} />
		{/each}
	</nav>

	{#if isAdmin}
		<!-- One entry; the sections live in the Admin area's own sub-nav. -->
		<nav class="nav admin-group" aria-label="Admin">
			{#if !collapsed}
				<p class="nav-group-label">Admin</p>
			{/if}
			<NavLink item={adminEntry} active={isActive('/admin', pathname)} collapsed={collapsed} />
		</nav>
	{/if}

	<div class="userchip-wrap">
		<Popover.Root>
			<Popover.Trigger>
				{#snippet child({ props })}
					<button class="userchip" {...props}>
						<span class="avatar" aria-hidden="true">{initials}</span>
						<span class="uchip-text" class:vh={collapsed}>
							<span class="uchip-name">{name}</span>
							<span class="uchip-role">{roleLabel(user.role)}</span>
						</span>
					</button>
				{/snippet}
			</Popover.Trigger>
			<Popover.Content side="top" align="start" sideOffset={8} trapFocus={false}>
				{#snippet child({ props, wrapperProps })}
					<div {...wrapperProps}>
						<div class="usermenu" {...props}>
							<div class="um-head">
								<p class="um-name">{name}</p>
								<p class="um-email">{user.email}</p>
								<p class="um-role">{roleLabel(user.role)}</p>
							</div>
							<hr class="rule" />
							<button class="um-item" type="button" onclick={onSignOut}>
								<Icon name="signout" />Sign out
							</button>
							{#if signOutError}
								<p class="um-error" role="alert">{signOutError}</p>
							{/if}
						</div>
					</div>
				{/snippet}
			</Popover.Content>
		</Popover.Root>
	</div>
</aside>

<style>
	.sidebar {
		background: var(--minerva-surface);
		border-right: var(--minerva-border-w) solid var(--minerva-border);
		display: flex;
		flex-direction: column;
		padding: 16px 12px 12px;
		min-height: 0;
	}

	@media (max-width: 899.98px) {
		.sidebar {
			display: none;
		}
	}

	.brand-row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 4px 8px 16px;
	}

	.sidebar.collapsed .brand-row {
		justify-content: center;
		padding-bottom: 12px;
	}

	.collapse-btn {
		width: 30px;
		height: 30px;
		display: grid;
		place-items: center;
		border-radius: 8px;
		color: var(--minerva-fg-muted);
	}

	.collapse-btn:hover {
		background: color-mix(in oklch, var(--minerva-fg) 6%, transparent);
		color: var(--minerva-fg);
	}

	.collapse-btn :global(svg) {
		transition: transform var(--minerva-motion-mid) ease;
	}

	.collapse-btn.collapsed :global(svg) {
		transform: rotate(180deg);
	}

	.nav {
		display: flex;
		flex-direction: column;
		gap: 2px;
		min-height: 0;
		overflow-y: auto;
	}

	.nav-group-label {
		font-size: 11px;
		font-weight: 600;
		letter-spacing: 0.08em;
		text-transform: uppercase;
		color: var(--minerva-fg-muted);
		padding: 18px 12px 6px;
		margin: 0;
	}

	.userchip-wrap {
		margin-top: auto;
		position: relative;
	}

	.userchip {
		width: 100%;
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 10px;
		border-radius: 10px;
		text-align: left;
	}

	.sidebar.collapsed .userchip {
		justify-content: center;
	}

	.userchip:hover {
		background: color-mix(in oklch, var(--minerva-fg) 6%, transparent);
	}

	.avatar {
		width: 32px;
		height: 32px;
		flex: 0 0 auto;
		border-radius: 50%;
		display: grid;
		place-items: center;
		border: var(--minerva-border-w) solid var(--minerva-border);
		background: color-mix(in oklch, var(--minerva-fg) 6%, transparent);
		font-size: 12px;
		font-weight: 600;
		color: var(--minerva-fg);
	}

	.uchip-text {
		min-width: 0;
	}

	.uchip-name {
		display: block;
		font-size: 13.5px;
		font-weight: 500;
		color: var(--minerva-fg);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.uchip-role {
		display: block;
		font-size: 12px;
		color: var(--minerva-fg-muted);
	}

	/* Placed by Bits UI floating-ui (side="top" align="start"); the wrapper
	   inherits this z-index from the content's computed style. */
	.usermenu {
		width: 240px;
		z-index: 40;
		background: var(--minerva-surface);
		border: var(--minerva-border-w) solid var(--minerva-border);
		border-radius: var(--minerva-radius);
		box-shadow: 0 12px 32px color-mix(in oklch, var(--minerva-fg) 14%, transparent);
		padding: var(--minerva-space-2);
	}

	.um-head {
		padding: 10px 12px 8px;
	}

	.um-name {
		font-size: 15px;
		font-weight: 600;
		color: var(--minerva-fg);
		margin: 0;
	}

	.um-email {
		font-size: 13px;
		color: var(--minerva-fg-muted);
		margin: 0;
	}

	.um-role {
		font-size: 12px;
		color: var(--minerva-fg-muted);
		margin: 2px 0 0;
	}

	.rule {
		border: 0;
		border-top: var(--minerva-border-w) solid var(--minerva-border);
		margin: 8px 4px;
	}

	.um-item {
		width: 100%;
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 9px 12px;
		border-radius: 8px;
		font-size: 14px;
		color: var(--minerva-fg);
		text-align: left;
	}

	.um-item:hover {
		background: color-mix(in oklch, var(--minerva-fg) 6%, transparent);
	}

	.um-item :global(svg) {
		color: var(--minerva-fg-muted);
	}

	.um-error {
		font-size: 13px;
		color: var(--minerva-accent);
		padding: 8px 12px 4px;
		margin: 0;
	}
</style>
