<script module>
  export const meta = {
    title: 'Components/ConversationList',
    component: ConversationList,
  };
</script>

<script lang="ts">
  import { Story, Template } from '@storybook/addon-svelte-csf';
  import ConversationList from './ConversationList.svelte';
  import type { Conversation } from '$lib/types';

  const sample: Conversation[] = [
    { id: '1', user_id: 'u1', agent_name: 'DevOps assistant', title: 'Rust async question', created_at: '2026-06-01T10:00:00Z', updated_at: '2026-06-01T10:05:00Z' },
    { id: '2', user_id: 'u1', agent_name: null, title: null, created_at: '2026-06-02T14:00:00Z', updated_at: '2026-06-02T14:00:00Z' },
    { id: '3', user_id: 'u1', agent_name: 'DevOps assistant', title: 'Kubernetes debug', created_at: '2026-06-03T09:00:00Z', updated_at: '2026-06-03T09:30:00Z' },
  ];
</script>

<Template let:args>
  <div class="w-64 h-96 bg-stone-900 border border-stone-700">
    <ConversationList {...args} onSelect={() => {}} onDelete={() => {}} onNew={() => {}} />
  </div>
</Template>

<Story name="Liste" args={{ conversations: sample, activeId: '1' }} />
<Story name="Vide" args={{ conversations: [] }} />
<Story name="Chargement" args={{ conversations: [], loading: true }} />
