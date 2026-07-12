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
    { name: 'DevOps assistant', description: 'Kubernetes expert', mode: 'primary', model: 'qwen2.5', toolsets: [], skills: 'auto', system_prompt: '' },
    { name: 'Rust helper', description: null, mode: 'primary', model: 'qwen2.5', toolsets: [], skills: 'auto', system_prompt: '' },
  ];
</script>

<Template let:args>
  <div class="w-64 bg-stone-900">
    <AgentSelector {...args} onChange={() => {}} />
  </div>
</Template>

<Story name="Agents disponibles" args={{ agents, value: 'DevOps assistant' }} />
<Story name="Sans sélection" args={{ agents, value: null }} />
<Story name="Désactivé" args={{ agents, value: 'DevOps assistant', disabled: true }} />
<Story name="Vide" args={{ agents: [], value: null }} />
