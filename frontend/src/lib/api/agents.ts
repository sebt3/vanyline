import { apiFetch } from './client';
import type { Agent } from '$lib/types';

export function listAgents(): Promise<Agent[]> {
  return apiFetch<Agent[]>('/api/agents');
}

export function getAgent(name: string): Promise<Agent> {
  return apiFetch<Agent>(`/api/agents/${name}`);
}
