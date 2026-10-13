// Client-side API helper for browser-initiated requests (the 6.4 auth screens,
// the 6.5 admin actions). These go through the browser instead of SvelteKit
// form actions or server loads on purpose: rate limits keyed on the client IP
// must see the real IP, and the response's Set-Cookie is handled by the
// browser and never forwarded. Same-origin only.

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

export async function apiRequest<T>(
	method: 'GET' | 'PUT' | 'POST' | 'DELETE',
	path: string,
	body?: unknown
): Promise<ApiResult<T>> {
	let res: Response;
	try {
		res = await fetch(path, {
			method,
			headers: body !== undefined ? { 'Content-Type': 'application/json' } : undefined,
			body: body !== undefined ? JSON.stringify(body) : undefined
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

export function postJson<T>(path: string, body: unknown): Promise<ApiResult<T>> {
	return apiRequest<T>('POST', path, body);
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
