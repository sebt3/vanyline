<script module>
  export const meta = {
    title: 'Components/AgentSelector',
    component: AgentSelector,
  };
</script>

<script lang="ts">
  import { Story, Template } from '@storybook/addon-svelte-csf';
  import AgentSelector from './AgentSelector.svelte';
  import type { Agent } from '$lib/types';

  const agents: Agent[] = [
    { id: 'a1', name: 'DevOps assistant', description: 'Kubernetes expert', system_prompt: '', mcp_servers: [], created_at: '', updated_at: '' },
    { id: 'a2', name: 'Rust helper', description: null, system_prompt: '', mcp_servers: [], created_at: '', updated_at: '' },
  ];
</script>

<Template let:args>
  <div class="w-64 bg-stone-900">
    <AgentSelector {...args} onChange={() => {}} />
  </div>
</Template>

<Story name="Agents disponibles" args={{ agents, value: 'a1' }} />
<Story name="Sans sélection" args={{ agents, value: null }} />
<Story name="Désactivé" args={{ agents, value: 'a1', disabled: true }} />
<Story name="Vide" args={{ agents: [], value: null }} />
