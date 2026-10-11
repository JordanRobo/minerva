<script lang="ts">
	import Icon from '../icons/Icon.svelte';
	import Input from './Input.svelte';
	import PasswordInput from './PasswordInput.svelte';

	let {
		id,
		label,
		hint,
		error,
		input = 'text',
		value = $bindable(''),
		...inputRest
	}: {
		id: string;
		label: string;
		hint?: string;
		error?: string;
		input?: 'text' | 'password';
		value?: string;
	} & Record<string, unknown> = $props();

	// The input points at this field's hint/error text so a screen reader
	// reads the message on focus.
	const describedBy = $derived(
		[hint ? `${id}-hint` : null, error ? `${id}-error` : null].filter(Boolean).join(' ') || undefined
	);
</script>

<div class="field">
	<label for={id}>{label}</label>
	{#if input === 'password'}
		<PasswordInput
			id={id}
			bind:value
			aria-describedby={describedBy}
			aria-invalid={error ? true : undefined}
			{...inputRest}
		/>
	{:else}
		<Input
			id={id}
			bind:value
			aria-describedby={describedBy}
			aria-invalid={error ? true : undefined}
			{...inputRest}
		/>
	{/if}
	{#if hint && !error}
		<p class="hint" id="{id}-hint">{hint}</p>
	{/if}
	{#if error}
		<!-- No error colour token exists: the message is fg text with an
		     icon and an accent left border — never colour alone. -->
		<p class="error" id="{id}-error">
			<Icon name="alert" size={14} />
			<span>{error}</span>
		</p>
	{/if}
</div>

<style>
	/* docs/design/app-shell.html .field, tokens only. */
	.field {
		display: flex;
		flex-direction: column;
		gap: 6px;
	}

	.field > label {
		font-size: 13px;
		font-weight: 500;
		color: var(--minerva-fg-muted);
	}

	.hint {
		margin: 0;
		font-size: 13px;
		color: var(--minerva-fg-muted);
	}

	.error {
		display: flex;
		align-items: center;
		gap: 6px;
		margin: 0;
		padding-left: 10px;
		border-left: 3px solid var(--minerva-accent);
		font-size: 13px;
		color: var(--minerva-fg);
	}
</style>
