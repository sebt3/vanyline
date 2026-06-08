import { apiFetch } from './client';
import type { User } from '$lib/types';

export function getMe(): Promise<User> {
  return apiFetch<User>('/api/me');
}
