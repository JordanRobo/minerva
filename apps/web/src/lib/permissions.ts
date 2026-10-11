import type { Role } from '$lib/server/api';

// Cosmetic only: the server enforces every permission (server/domain
// role.rs, `Role::allows`). This mirrors that matrix — the same three
// actions, the same strings — so the UI can hide controls a role cannot
// use. Never rely on it for access control.
export type Action = 'view' | 'edit' | 'manage_users';

export function can(role: Role, action: Action): boolean {
	if (action === 'view') return true; // admin, staff and read_only
	if (action === 'edit') return role === 'admin' || role === 'staff';
	return role === 'admin'; // manage_users
}
