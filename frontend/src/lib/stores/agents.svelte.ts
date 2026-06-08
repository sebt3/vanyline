import { listAgents } from '$lib/api/agents';
import type { Agent } from '$lib/types';

let agents = $state<Agent[]>([]);
let loading = $state(false);

export const agentsStore = {
  get agents() { return agents; },
  get loading() { return loading; },
  async load() {
    loading = true;
    try {
      agents = await listAgents();
    } catch {
      agents = [];
    } finally {
      loading = false;
    }
  },
};
