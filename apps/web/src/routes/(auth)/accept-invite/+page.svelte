<script lang="ts">
	import { onMount, tick } from 'svelte';
	import { goto } from '$app/navigation';
	import { postJson } from '$lib/api';
	import { postErrorMessage } from '$lib/auth-errors';
	import Button from '$lib/components/ui/Button.svelte';
	import Field from '$lib/components/ui/Field.svelte';
	import FormAlert from '$lib/components/ui/FormAlert.svelte';
	import Input from '$lib/components/ui/Input.svelte';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import { roleLabel } from '$lib/nav';
	import type { TokenLinkInfo } from '$lib/server/api';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();

	// Must match the server's MIN_PASSWORD_LENGTH
	// (server/application/src/bootstrap.rs) — the server re-checks either way.
	const MIN_PASSWORD_LENGTH = 8;

	type Phase = 'checking' | 'invalid' | 'retryable' | 'form' | 'account-exists';

	let phase = $state<Phase>('checking');
	let info = $state<TokenLinkInfo | null>(null);
	let retryMessage = $state('');

	let displayName = $state('');
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
		} else if (res.data.purpose === 'invite') {
			info = res.data;
			phase = 'form';
		} else {
			// A password-reset token opened on this page.
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
			document.getElementById(errors.password ? 'invite-password' : 'invite-confirm')?.focus();
			return;
		}

		submitting = true;
		const res = await postJson('/api/auth/accept-invite', {
			token: data.token,
			password,
			// Blank name → the server falls back to the email's local part.
			display_name: displayName.trim() ? displayName.trim() : null
		});
		submitting = false;

		if (res.ok) {
			// Accepting the invite also signs the new account in (session
			// cookie), so a normal navigation straight into the app.
			await goto('/', { invalidateAll: true });
			return;
		}
		if (res.status === 409) {
			phase = 'account-exists';
			return;
		}
		formError = postErrorMessage(res);
		await tick();
		document.querySelector<HTMLElement>('form [role="alert"]')?.focus();
	}
</script>

<svelte:head>
	<title>Join Minerva · Minerva</title>
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
{:else if phase === 'account-exists'}
	<h1>An account already exists</h1>
	<p class="lede">An account with this email already exists. Try signing in instead.</p>
	<div class="options">
		<Button variant="ghost" href="/login">Go to sign in</Button>
	</div>
{:else if phase === 'form' && info && info.role}
	<h1>Welcome to Minerva</h1>
	<p class="lede">You've been invited to join as {roleLabel(info.role)}.</p>
	<div class="options">
		<form novalidate aria-busy={submitting || undefined} onsubmit={handleSubmit}>
			{#if formError}
				<FormAlert>{formError}</FormAlert>
			{/if}
			<Field id="invite-email" label="Email" value={info.email} readonly />
			<!-- Autofill anchor for password managers: in the document for
			     autocomplete only, never visible, never keyboard-reachable,
			     its value is never sent. -->
			<Input
				class="visually-hidden"
				autocomplete="username"
				aria-hidden="true"
				tabindex={-1}
			/>
			<Field
				id="invite-name"
				label="Your name"
				hint="Optional"
				bind:value={displayName}
				autocomplete="name"
			/>
			<Field
				id="invite-password"
				label="New password"
				hint={`At least ${MIN_PASSWORD_LENGTH} characters.`}
				error={fieldErrors.password}
				input="password"
				bind:value={password}
				autocomplete="new-password"
			/>
			<Field
				id="invite-confirm"
				label="Confirm password"
				error={fieldErrors.confirm}
				input="password"
				bind:value={confirm}
				autocomplete="new-password"
			/>
			<Button variant="primary" type="submit" disabled={submitting}>
				{submitting ? 'Creating your account...' : 'Create my account'}
			</Button>
		</form>
		<Button variant="ghost" href="/login">Back to sign in</Button>
	</div>
{/if}
