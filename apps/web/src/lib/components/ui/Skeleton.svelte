<script lang="ts">
	// Generic skeleton primitives: a line (flexes to fill its row) or a
	// circle. Compose them into rows; PageSkeleton is the one fixed
	// composition for now.
	let { variant = 'line', width }: { variant?: 'line' | 'circle'; width?: string } = $props();
</script>

<span class="sk {variant}" style={width ? `max-width: ${width};` : undefined} aria-hidden="true"></span>

<style>
	.sk {
		display: block;
		background: color-mix(in oklch, var(--minerva-fg) 9%, transparent);
	}

	.line {
		height: 12px;
		border-radius: 6px;
		flex: 1;
	}

	.circle {
		width: 36px;
		height: 36px;
		border-radius: 50%;
		flex: 0 0 auto;
	}
</style>
