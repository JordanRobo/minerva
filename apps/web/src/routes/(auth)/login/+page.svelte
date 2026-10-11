<script lang="ts">
	import { tick } from 'svelte';
	import { goto } from '$app/navigation';
	import { postJson } from '$lib/api';
	import { postErrorMessage, ssoErrorMessage } from '$lib/auth-errors';
	import Banner from '$lib/components/ui/Banner.svelte';
	import Button from '$lib/components/ui/Button.svelte';
	import Field from '$lib/components/ui/Field.svelte';
	import FormAlert from '$lib/components/ui/FormAlert.svelte';
	import type { AuthProvider } from '$lib/server/api';
	import type { PageData } from './$types';

	let { data }: { data: PageData } = $props();

	// The credentials-kind provider is what makes the email/password form
	// real; each redirect-kind provider becomes one full-width button.
	const credentials = $derived(data.providers.find((p) => p.kind === 'credentials'));
	const redirects = $derived(data.providers.filter((p) => p.kind === 'redirect'));

	let email = $state('');
	let password = $state('');
	let submitting = $state(false);
	let formError = $state<string | null>(null);
	let fieldErrors = $state<{ email?: string; password?: string }>({});

	function providerHref(provider: AuthProvider): string {
		// Full page navigation (data-sveltekit-reload): the API's /api/* link
		// is not a SvelteKit route.
		const sep = provider.login_url.includes('?') ? '&' : '?';
		return `${provider.login_url}${sep}next=${encodeURIComponent(data.next)}`;
	}

	async function handleSubmit(event: SubmitEvent) {
		event.preventDefault();
		if (submitting) return;

		// Client-side checks only; the server re-checks everything. The
		// messages never say which part of the server check failed.
		const errors: { email?: string; password?: string } = {};
		if (!email.trim()) errors.email = 'Enter your email address.';
		else if (!email.includes('@')) errors.email = 'Enter a valid email address.';
		if (!password) errors.password = 'Enter your password.';
		fieldErrors = errors;
		formError = null;
		if (errors.email || errors.password) {
			document.getElementById(errors.email ? 'login-email' : 'login-password')?.focus();
			return;
		}

		submitting = true;
		const res = await postJson('/api/auth/login', { email: email.trim(), password });
		submitting = false;

		if (res.ok) {
			await goto(data.next, { invalidateAll: true });
			return;
		}

		// 401 gets its own wording; everything else is generalised (never
		// which part failed).
		formError = res.status === 401 ? "That email and password don't match." : postErrorMessage(res);
		await tick();
		// The alert just rendered (formError is set); move focus to it. It is
		// the only role="alert" inside the form (field errors are plain <p>).
		document.querySelector<HTMLElement>('form [role="alert"]')?.focus();
	}
</script>

<svelte:head>
	<title>Sign in · Minerva</title>
</svelte:head>

<h1>Sign in to Minerva</h1>
<p class="lede">Use the account your school administrator set up for you.</p>

{#if data.notice === 'password_reset'}
	<Banner>Your password has been updated. Sign in with your new password.</Banner>
{/if}

{#if data.error}
	<div class="top">
		<FormAlert>
			{ssoErrorMessage(data.error)}
			<span class="ref">Reference: {data.error}</span>
		</FormAlert>
	</div>
{/if}

{#if data.providers.length === 0}
	<p class="lede options">Sign-in isn't set up yet. Contact your administrator.</p>
{:else}
	<div class="options">
		{#if credentials}
			<form novalidate aria-busy={submitting || undefined} onsubmit={handleSubmit}>
				{#if formError}
					<FormAlert>{formError}</FormAlert>
				{/if}
				<Field
					id="login-email"
					label="Email"
					error={fieldErrors.email}
					bind:value={email}
					autocomplete="username"
					autocapitalize="none"
					spellcheck={false}
				/>
				<Field
					id="login-password"
					label="Password"
					error={fieldErrors.password}
					input="password"
					bind:value={password}
					autocomplete="current-password"
				/>
				<Button variant="primary" type="submit" disabled={submitting}>
					{submitting ? 'Signing in...' : 'Sign in'}
				</Button>
			</form>
		{/if}

		{#if credentials && redirects.length > 0}
			<div class="divider" aria-hidden="true"><span>or</span></div>
		{/if}

		{#each redirects as provider (provider.id)}
			<Button
				variant="secondary"
				href={providerHref(provider)}
				data-sveltekit-reload={true}
			>
				Sign in with {provider.display_name}
			</Button>
		{/each}

		{#if credentials}
			<p class="helper">
				Forgot your password? Ask your school's administrator to send you a reset link.
			</p>
		{/if}
	</div>
{/if}

<style>
	.top {
		margin-top: var(--minerva-space-4);
	}

	.divider {
		display: flex;
		align-items: center;
		gap: 10px;
		font-size: 12px;
		color: var(--minerva-fg-muted);
	}

	.divider::before,
	.divider::after {
		content: '';
		flex: 1;
		height: var(--minerva-border-w);
		background: var(--minerva-border);
	}

	.helper {
		margin: 0;
		font-size: 13px;
		color: var(--minerva-fg-muted);
	}

	.ref {
		display: block;
		margin-top: 4px;
		font-size: 12px;
		color: var(--minerva-fg-muted);
	}
</style>
