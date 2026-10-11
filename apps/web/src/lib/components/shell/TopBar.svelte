<script lang="ts">
	import Wordmark from './Wordmark.svelte';

	let { initials, onOpenSheet }: { initials: string; onOpenSheet: () => void } = $props();
</script>

<header class="topbar">
	<Wordmark small />
	<button class="avatar-btn" type="button" aria-label="Account menu" onclick={onOpenSheet}>
		<span class="avatar" aria-hidden="true">{initials}</span>
	</button>
</header>

<style>
	.topbar {
		flex: 0 0 auto;
		height: var(--minerva-shell-topbar-h);
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 0 var(--minerva-space-2) 0 var(--minerva-space-4);
		background: var(--minerva-surface);
		border-bottom: var(--minerva-border-w) solid var(--minerva-border);
	}

	@media (min-width: 900px) {
		.topbar {
			display: none;
		}
	}

	.avatar-btn {
		width: var(--minerva-control-h);
		height: var(--minerva-control-h);
		display: grid;
		place-items: center;
		border-radius: 50%;
	}

	.avatar-btn:hover {
		background: color-mix(in oklch, var(--minerva-fg) 6%, transparent);
	}

	.avatar {
		width: 34px;
		height: 34px;
		border-radius: 50%;
		display: grid;
		place-items: center;
		border: var(--minerva-border-w) solid var(--minerva-border);
		background: color-mix(in oklch, var(--minerva-fg) 6%, transparent);
		font-size: 12px;
		font-weight: 600;
	}
</style>
