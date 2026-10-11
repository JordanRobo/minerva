// Validates a `next` value (e.g. /login?next=/goals) so it can only ever
// navigate within this origin. The same rule the server applies to its own
// `next` handling (see server/interface/src/redirect.rs): one leading `/`,
// not `//` (protocol-relative — that would pick a foreign host), no
// backslash, no scheme (impossible once the leading-`/` check passes, but
// kept explicit), no control or whitespace characters, bounded length, and
// nothing that points back at an auth screen (a `next` of /login would loop;
// the token pages take no `next`).

const AUTH_PREFIXES = ['/login', '/accept-invite', '/reset-password'] as const;

export function safeNext(raw: unknown, fallback = '/'): string {
	if (typeof raw !== 'string' || raw.length === 0 || raw.length > 512) return fallback;
	if (!raw.startsWith('/') || raw.startsWith('//')) return fallback;
	if (raw.includes('\\')) return fallback;
	if (/[^\x21-\x7e]/.test(raw)) return fallback; // control chars, whitespace, non-ASCII
	if (AUTH_PREFIXES.some((prefix) => raw.startsWith(prefix))) return fallback;
	return raw;
}
