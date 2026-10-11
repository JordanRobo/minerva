// Client-side API helper for the auth screens (roadmap 6.4). Auth POSTs go
// through the browser instead of SvelteKit form actions on purpose: login and
// token-link rate limits are keyed on the client IP, so a call made from the
// web container would pool every user into one bucket. Same-origin only; the
// response's Set-Cookie is handled by the browser and never forwarded.

export type ApiOk<T> = { ok: true; status: number; data: T };
export type ApiErr = {
	ok: false;
	status: number;
	code: string;
	message: string;
	/** Seconds until the rate limiter's window resets (429 only). */
	retryAfter?: number;
};
export type ApiResult<T> = ApiOk<T> | ApiErr;

export async function postJson<T>(path: string, body: unknown): Promise<ApiResult<T>> {
	let res: Response;
	try {
		res = await fetch(path, {
			method: 'POST',
			headers: { 'Content-Type': 'application/json' },
			body: JSON.stringify(body)
		});
	} catch {
		return { ok: false, status: 0, code: 'network', message: 'network' };
	}

	const text = await res.text();
	const json: unknown = text ? safeParse(text) : null;

	if (!res.ok || json === undefined) {
		// Non-JSON bodies are treated as a network-level failure (code
		// 'network') so components never have to parse error envelopes.
		const err =
			json && typeof json === 'object' && 'error' in json
				? (json as { error?: { code?: unknown; message?: unknown } }).error
				: undefined;
		return {
			ok: false,
			status: res.status,
			code: typeof err?.code === 'string' ? err.code : 'network',
			message: typeof err?.message === 'string' ? err.message : text,
			retryAfter: parseRetryAfter(res.headers.get('Retry-After'))
		};
	}

	return { ok: true, status: res.status, data: json as T };
}

function safeParse(text: string): unknown {
	try {
		return JSON.parse(text);
	} catch {
		return undefined;
	}
}

function parseRetryAfter(header: string | null): number | undefined {
	if (header === null) return undefined;
	const n = Number(header);
	return Number.isFinite(n) ? n : undefined;
}
