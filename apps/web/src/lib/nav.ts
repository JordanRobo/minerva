import type { IconName } from '$lib/components/icons/Icon.svelte';
import type { Role } from '$lib/server/api';

export interface NavItem {
	href: string;
	label: string;
	icon: IconName;
}

export const primaryNav: NavItem[] = [
	{ href: '/dashboard', label: 'Dashboard', icon: 'dashboard' },
	{ href: '/goals', label: 'Goals', icon: 'goal' },
	{ href: '/milestones', label: 'Milestones', icon: 'milestone' },
	{ href: '/tasks', label: 'Tasks', icon: 'task' },
	{ href: '/my-tasks', label: 'My tasks', icon: 'my' }
];

export const adminNav: NavItem[] = [
	{ href: '/admin/users', label: 'Users', icon: 'users' },
	{ href: '/admin/invites', label: 'Invites', icon: 'invite' },
	{ href: '/admin/sso', label: 'Single sign-on', icon: 'sso' }
];

/** Plain-language role names, as shown to users (API sends snake_case). */
export function roleLabel(role: Role): string {
	if (role === 'admin') return 'Administrator';
	if (role === 'staff') return 'Staff';
	return 'Read-only';
}

/** /goals activates Goals, /goals/123 does too; /my-tasks does not activate Tasks. */
export function isActive(href: string, pathname: string): boolean {
	return pathname === href || pathname.startsWith(`${href}/`);
}
