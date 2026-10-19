<script lang="ts">
	import { Toggle } from 'bits-ui';
	import Input from './Input.svelte';
	import Icon from '../icons/Icon.svelte';

	let {
		value = $bindable(''),
		...rest
	}: { value?: string } & Record<string, unknown> = $props();

	let visible = $state(false);
</script>

<div class="pw">
	<Input type={visible ? 'text' : 'password'} bind:value {...rest} />
	<!-- Bits UI Toggle (D18): the child snippet keeps the <button> in this
	     component's scope so the .toggle styles apply; `class` and
	     `aria-label` flow back through props, aria-pressed is set by the
	     primitive, and its click handler flips the bound pressed state. -->
	<Toggle.Root bind:pressed={visible} class="toggle" aria-label={visible ? 'Hide password' : 'Show password'}>
		{#snippet child({ props })}
			<button {...props}>
				<Icon name={visible ? 'eye-off' : 'eye'} size={18} />
			</button>
		{/snippet}
	</Toggle.Root>
</div>

<style>
	/* The toggle sits inside the field's right edge; the input leaves room
	   for it so typed text never slides under the button. */
	.pw {
		position: relative;
	}

	.pw :global(input) {
		padding-right: var(--minerva-control-h);
	}

	.toggle {
		position: absolute;
		top: 0;
		right: 0;
		width: var(--minerva-control-h);
		height: 100%;
		display: flex;
		align-items: center;
		justify-content: center;
		border: 0;
		border-radius: 0 var(--minerva-radius) var(--minerva-radius) 0;
		background: transparent;
		color: var(--minerva-fg-muted);
		cursor: pointer;
	}

	.toggle:hover {
		color: var(--minerva-fg);
		background: color-mix(in oklch, var(--minerva-fg) 6%, transparent);
	}
</style>
