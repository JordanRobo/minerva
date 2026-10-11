<script lang="ts">
	import { onMount, tick } from 'svelte';
	import { goto } from '$app/navigation';
	import { postJson } from '$lib/api';
	import { postErrorMessage } from '$lib/auth-errors';
	import Button from '$lib/components/ui/Button.svelte';
	import Field from '$lib/components/ui/Field.svelte';
	import FormAlert from '$lib/components/ui/FormAlert.svelte';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import type { TokenLinkInfo } from '$lib/server/api';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();

	// Must match the server's MIN_PASSWORD_LENGTH
	// (server/application/src/bootstrap.rs) — the server re-checks either way.
	const MIN_PASSWORD_LENGTH = 8;

	type Phase = 'checking' | 'invalid' | 'retryable' | 'form';

	let phase = $state<Phase>('checking');
	let info = $state<TokenLinkInfo | null>(null);
	let retryMessage = $state('');

	let password = $state('');
	let confirm = $state('');
	let submitting = $state(false);
	let formError = $state<string | null>(null);
	let fieldErrors = $state<{ password?: string; confirm?: string }>({});

	async function checkLink() {
		phase = 'checking';
		formError = null;
		if (!data.token) {
			phase = 'invalid';
			return;
		}
		const res = await postJson<TokenLinkInfo>('/api/auth/tokens/inspect', { token: data.token });
		if (!res.ok) {
			if (res.code === 'network' || res.code === 'rate_limited') {
				retryMessage = postErrorMessage(res);
				phase = 'retryable';
			} else {
				// Unknown, expired, used or revoked tokens all answer 400
				// invalid_token — one state for all of them.
				phase = 'invalid';
			}
		} else if (res.data.purpose === 'password_reset') {
			info = res.data;
			phase = 'form';
		} else {
			// An invite token opened on this page.
			phase = 'invalid';
		}
	}

	onMount(() => {
		checkLink();
	});

	async function handleSubmit(event: SubmitEvent) {
		event.preventDefault();
		if (submitting) return;

		const errors: { password?: string; confirm?: string } = {};
		if (password.length < MIN_PASSWORD_LENGTH) {
			errors.password = `Your password must be at least ${MIN_PASSWORD_LENGTH} characters.`;
		}
		if (confirm !== password) errors.confirm = "Passwords don't match.";
		fieldErrors = errors;
		formError = null;
		if (errors.password || errors.confirm) {
			document.getElementById(errors.password ? 'reset-password' : 'reset-confirm')?.focus();
			return;
		}

		submitting = true;
		const res = await postJson('/api/auth/reset-password', { token: data.token, password });
		submitting = false;

		if (res.ok) {
			// The API revokes every session of the account on reset, so the
			// only sensible destination is the sign-in screen, with its
			// "password updated" notice.
			await goto('/login?notice=password_reset');
			return;
		}
		formError = postErrorMessage(res);
		await tick();
		document.querySelector<HTMLElement>('form [role="alert"]')?.focus();
	}
</script>

<svelte:head>
	<title>Reset your password · Minerva</title>
</svelte:head>

{#if phase === 'checking'}
	<div class="checking" aria-busy="true" aria-label="Checking your link">
		<Skeleton variant="line" />
		<Skeleton variant="line" width="70%" />
		<Skeleton variant="line" width="40%" />
	</div>
{:else if phase === 'invalid'}
	<h1>This link isn't valid any more</h1>
	<p class="lede">
		It may have expired, been used already, or been replaced by a newer one. Ask your school's
		administrator to send you a new one.
	</p>
	<div class="options">
		<Button variant="ghost" href="/login">Go to sign in</Button>
	</div>
{:else if phase === 'retryable'}
	<h1>Please try again</h1>
	<div class="options">
		<FormAlert>{retryMessage}</FormAlert>
		<Button variant="ghost" onclick={() => checkLink()}>Try again</Button>
	</div>
{:else if phase === 'form' && info}
	<h1>Reset your password</h1>
	<p class="lede">Choose a new password for {info.email}.</p>
	<div class="options">
		<form novalidate aria-busy={submitting || undefined} onsubmit={handleSubmit}>
			{#if formError}
				<FormAlert>{formError}</FormAlert>
			{/if}
			<Field
				id="reset-password"
				label="New password"
				hint={`At least ${MIN_PASSWORD_LENGTH} characters.`}
				error={fieldErrors.password}
				input="password"
				bind:value={password}
				autocomplete="new-password"
			/>
			<Field
				id="reset-confirm"
				label="Confirm password"
				error={fieldErrors.confirm}
				input="password"
				bind:value={confirm}
				autocomplete="new-password"
			/>
			<Button variant="primary" type="submit" disabled={submitting}>
				{submitting ? 'Updating...' : 'Update password'}
			</Button>
		</form>
		<Button variant="ghost" href="/login">Back to sign in</Button>
	</div>
{/if}
