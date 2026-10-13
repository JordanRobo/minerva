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

/** The single Admin entry in the main navigation (admins only). */
export const adminEntry: NavItem = { href: '/admin', label: 'Admin', icon: 'shield' };

/** The sub-navigation inside the Admin area (`(app)/admin/+layout.svelte`). */
export const adminSections: NavItem[] = [
	{ href: '/admin/people', label: 'People', icon: 'users' },
	{ href: '/admin/invitations', label: 'Invitations', icon: 'invite' },
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
