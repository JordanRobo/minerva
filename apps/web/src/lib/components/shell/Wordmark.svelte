<script lang="ts">
	let {
		collapsed = false,
		small = false,
		big = false,
		ink = false
	}: { collapsed?: boolean; small?: boolean; big?: boolean; ink?: boolean } = $props();
</script>

<!-- The wordmark is the official artwork from /static (path-based, so it can't
     drift from the brand — a text rendering once gained a stray i: "Minervia").
     html.dark — or `ink`, on the always-dark rail — swaps in the reversed
     variant. -->
<span class="brand" class:small={small && !big} class:big={big} class:ink={ink} aria-label="Minerva">
	{#if collapsed}
		<span class="sym">M<span class="tdot">.</span></span>
	{:else}
		<img class="wm wm-light" src="/wordmark.svg" alt="" />
		<img class="wm wm-dark" src="/wordmark-reversed.svg" alt="" />
	{/if}
</span>

<style>
	/* The collapsed "M." is set in Fraunces (not the UI display token) so the
	   text mark matches the official SVG artwork. */
	.brand {
		font-family: "Fraunces";
		font-weight: 600;
		font-size: 21px;
		line-height: 1;
		color: inherit;
	}

	/* On the always-dark rail, the reversed artwork and paper text. */
	.brand.ink {
		color: var(--minerva-rail-fg);
	}

	.brand.ink .wm-light {
		display: none;
	}

	.brand.ink .wm-dark {
		display: block;
	}

	.brand.small {
		font-size: 17px;
	}

	/* The bare pre-sign-in layout uses the 28px wordmark from the
	   prototype's auth frame. */
	.brand.big {
		font-size: 28px;
	}

	/* The artwork fills its viewBox, so em sizing tracks Fraunces' cap
	   height (~0.7em) at every size. */
	.wm {
		display: block;
		height: 0.7em;
		width: auto;
	}

	.wm-dark {
		display: none;
	}

	:global(html.dark) .wm-light {
		display: none;
	}

	:global(html.dark) .wm-dark {
		display: block;
	}

	.tdot {
		color: var(--minerva-dot);
	}
</style>
