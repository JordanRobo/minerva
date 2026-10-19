<script lang="ts">
	import { Tooltip } from 'bits-ui';
	import { page } from '$app/state';
	import { beforeNavigate, goto } from '$app/navigation';
	import Sidebar from '$lib/components/shell/Sidebar.svelte';
	import TopBar from '$lib/components/shell/TopBar.svelte';
	import TabBar from '$lib/components/shell/TabBar.svelte';
	import UserSheet from '$lib/components/shell/UserSheet.svelte';
	import { adminEntry, primaryNav } from '$lib/nav';
	import type { LayoutProps } from './$types';

	let { data, children } = $props();

	// $derived, not const: `data` is replaced on revalidation (e.g. a role
	// change made elsewhere) and the shell must follow it.
	const user = $derived(data.user);

	// The mobile tab bar gains an Admin entry for admins (5 → 6 tabs).
	const tabs = $derived(user.role === 'admin' ? [...primaryNav, adminEntry] : primaryNav);

	const initials = $derived.by(() => {
		const src = user.display_name.trim() || user.email;
		const parts = src.split(/\s+/).filter(Boolean);
		if (parts.length >= 2) return (parts[0][0] + parts[parts.length - 1][0]).toUpperCase();
		return src.slice(0, 2).toUpperCase();
	});

	// Sidebar collapse is in-memory only: a reload starts expanded.
	let collapsed = $state(false);

	let sheetOpen = $state(false);

	let signingOut = $state(false);
	let signOutError = $state<string | null>(null);

	// Navigating anywhere closes the mobile sheet (its links navigate too).
	// beforeNavigate, not an $effect on the path: an effect that also reads
	// sheetOpen re-runs when the sheet opens and closes it again.
	beforeNavigate(() => {
		sheetOpen = false;
	});

	async function signOut() {
		if (signingOut) return;
		signingOut = true;
		signOutError = null;
		try {
			// Browser-side, through the same origin: Caddy (or the Vite dev
			// proxy) forwards /api/* to the API. No body — the server only
			// needs the session cookie.
			const res = await fetch('/api/auth/logout', { method: 'POST' });
			if (!res.ok) throw new Error(`logout failed: ${res.status}`);
			sheetOpen = false;
			await goto('/login', { invalidateAll: true });
		} catch {
			signOutError = 'Couldn\'t sign out. Please try again.';
		} finally {
			signingOut = false;
		}
	}
</script>

<!-- One provider for the whole shell (D18): coordinates at most one visible
     tooltip at a time and supplies the hover delay. -->
<Tooltip.Provider>
<div class="shell" class:collapsed={collapsed}>
	<a class="skip-link" href="#main">Skip to content</a>

	<Sidebar
		user={user}
		initials={initials}
		isAdmin={user.role === 'admin'}
		collapsed={collapsed}
		pathname={page.url.pathname}
		signOutError={signOutError}
		onToggleCollapse={() => (collapsed = !collapsed)}
		onSignOut={signOut}
	/>

	<TopBar initials={initials} onOpenSheet={() => (sheetOpen = true)} />

	<main id="main" class="content" tabindex="-1">
		<div class="content-inner">
			{@render children()}
		</div>
	</main>

	<TabBar pathname={page.url.pathname} items={tabs} />

	<UserSheet
		user={user}
		initials={initials}
		open={sheetOpen}
		signOutError={signOutError}
		onClosed={() => (sheetOpen = false)}
		onSignOut={signOut}
	/>
</div>
</Tooltip.Provider>

<style>
	/* One breakpoint (900px): desktop is a two-column grid with a fixed
	   sidebar and only the content column scrolling; below it, a 56px top
	   bar and a 64px (+ safe area) bottom tab bar around the same content. */
	.shell {
		display: flex;
		flex-direction: column;
		height: 100dvh;
		overflow: hidden;
		background: var(--minerva-bg);
		color: var(--minerva-fg);
	}

	@media (min-width: 900px) {
		.shell {
			display: grid;
			grid-template-columns: var(--minerva-shell-sidebar-w) minmax(0, 1fr);
			grid-template-rows: 1fr;
		}

		.shell.collapsed {
			grid-template-columns: var(--minerva-shell-sidebar-w-min) minmax(0, 1fr);
		}
	}

	.content {
		flex: 1;
		min-width: 0;
		min-height: 0;
		overflow-y: auto;
		overflow-x: hidden;
	}

	.content-inner {
		max-width: var(--minerva-shell-page-max);
		margin: 0 auto;
		padding: var(--minerva-space-4);
	}

	@media (min-width: 900px) {
		.content-inner {
			padding: var(--minerva-space-6);
		}
	}
</style>
