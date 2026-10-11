<script lang="ts">
	import Icon from '$lib/components/icons/Icon.svelte';
	import { adminNav, isActive, primaryNav, roleLabel } from '$lib/nav';
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

	// Desktop user menu: a disclosure (aria-expanded on the trigger), not a
	// role="menu" popup — the panel holds one action, disclosure is the
	// honest pattern.
	let menuOpen = $state(false);
	let chipBtn = $state<HTMLButtonElement | null>(null);
	let menuEl = $state<HTMLDivElement | null>(null);
	let lastFocus: HTMLElement | null = null;

	function toggleMenu() {
		menuOpen = !menuOpen;
		if (menuOpen) lastFocus = (document.activeElement as HTMLElement | null) ?? null;
	}

	function closeMenu(refocus = true) {
		if (!menuOpen) return;
		menuOpen = false;
		if (refocus) lastFocus?.focus();
	}

	$effect(() => {
		if (!menuOpen) return;
		const onKey = (e: KeyboardEvent) => {
			if (e.key === 'Escape') {
				e.preventDefault();
				e.stopPropagation();
				closeMenu();
			}
		};
		const onClick = (e: MouseEvent) => {
			const t = e.target as Node;
			if (menuEl && !menuEl.contains(t) && chipBtn && !chipBtn.contains(t)) closeMenu();
		};
		document.addEventListener('keydown', onKey, true);
		document.addEventListener('mousedown', onClick, true);
		return () => {
			document.removeEventListener('keydown', onKey, true);
			document.removeEventListener('mousedown', onClick, true);
		};
	});
</script>

<aside class="sidebar" class:collapsed={collapsed}>
	<div class="brand-row">
		<Wordmark collapsed={collapsed} />
		<button
			class="collapse-btn"
			class:collapsed={collapsed}
			type="button"
			aria-label={collapsed ? 'Expand sidebar' : 'Collapse sidebar'}
			aria-expanded={!collapsed}
			title={collapsed ? 'Expand' : 'Collapse'}
			onclick={onToggleCollapse}
		>
			<Icon name="chevron" size={16} />
		</button>
	</div>

	<nav class="nav" aria-label="Primary">
		{#each primaryNav as item (item.href)}
			<NavLink item={item} active={isActive(item.href, pathname)} collapsed={collapsed} />
		{/each}
	</nav>

	{#if isAdmin}
		<nav class="nav admin-group" aria-label="Admin">
			{#if !collapsed}
				<p class="nav-group-label">Admin</p>
			{/if}
			{#each adminNav as item (item.href)}
				<NavLink item={item} active={isActive(item.href, pathname)} collapsed={collapsed} />
			{/each}
		</nav>
	{/if}

	<div class="userchip-wrap">
		<button
			bind:this={chipBtn}
			class="userchip"
			type="button"
			aria-haspopup="true"
			aria-expanded={menuOpen}
			aria-controls="user-menu"
			onclick={toggleMenu}
		>
			<span class="avatar" aria-hidden="true">{initials}</span>
			<span class="uchip-text" class:vh={collapsed}>
				<span class="uchip-name">{name}</span>
				<span class="uchip-role">{roleLabel(user.role)}</span>
			</span>
		</button>

		{#if menuOpen}
			<div class="usermenu" id="user-menu" bind:this={menuEl}>
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
		{/if}
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

	.usermenu {
		position: absolute;
		bottom: calc(100% + 8px);
		left: 0;
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
