<script lang="ts">
	import BareLayout from '$lib/components/shell/BareLayout.svelte';
	import '$lib/styles/auth.css';
	import type { LayoutProps } from './$types';

	// Pre-sign-in routes (today just /login) render in the bare frame,
	// outside the (app) shell. The (auth) group is invisible in URLs.
	let { children } = $props();
</script>

<BareLayout>
	{@render children()}
</BareLayout>
