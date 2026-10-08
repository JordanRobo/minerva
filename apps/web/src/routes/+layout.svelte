<script lang="ts">
	import '$lib/styles/tokens.css';
	import '$lib/styles/shell-tokens.css';
	import '$lib/styles/fonts.css';
	import '$lib/styles/base.css';

	let { children } = $props();
</script>

<svelte:head>
	<link rel="icon" href="/favicon.svg" />
</svelte:head>

{@render children()}
