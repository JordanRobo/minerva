<script lang="ts">
	let {
		value = $bindable(''),
		type = 'text',
		...rest
	}: { value?: string; type?: string } & Record<string, unknown> = $props();
</script>

<input class="input" {type} bind:value {...rest} />

<style>
	/* docs/design/app-shell.html .field/.input, tokens only. */
	.input {
		width: 100%;
		min-height: var(--minerva-control-h);
		padding: 10px 14px;
		border: var(--minerva-border-w) solid var(--minerva-border);
		border-radius: var(--minerva-radius);
		background: var(--minerva-surface);
		color: var(--minerva-fg);
		font-family: inherit;
		font-size: 15px;
	}

	.input::placeholder {
		color: var(--minerva-fg-muted);
	}

	.input:focus {
		outline: 2px solid var(--minerva-accent);
		border-color: var(--minerva-accent);
	}

	.input[aria-invalid='true'] {
		border-color: var(--minerva-accent);
	}
</style>
