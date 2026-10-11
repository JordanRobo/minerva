<script lang="ts">
	import Skeleton from './Skeleton.svelte';
</script>

<div class="card" aria-busy="true" aria-label="Loading content">
	<div class="row">
		<Skeleton variant="circle" />
		<Skeleton variant="line" />
	</div>
	<div class="row">
		<Skeleton variant="circle" />
		<Skeleton variant="line" width="50%" />
	</div>
	<div class="row">
		<Skeleton variant="circle" />
		<Skeleton variant="line" />
	</div>
	<div class="row">
		<Skeleton variant="circle" />
		<Skeleton variant="line" width="50%" />
	</div>
</div>

<style>
	/* The whole card pulses, as in the prototype. The 1.5 s rhythm is a
	   component-local constant (not a token); base.css's global
	   prefers-reduced-motion rule stops the animation. */
	.card {
		padding: 24px;
		background: var(--minerva-surface);
		border: var(--minerva-border-w) solid var(--minerva-border);
		border-radius: var(--minerva-radius);
		animation: skpulse 1.5s ease-in-out infinite;
	}

	@keyframes skpulse {
		0%,
		100% {
			opacity: 1;
		}
		50% {
			opacity: 0.45;
		}
	}

	.row {
		display: flex;
		align-items: center;
		gap: 12px;
		margin-top: 18px;
	}
</style>
