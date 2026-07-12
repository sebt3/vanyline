<script lang="ts">
  import type { Agent } from '$lib/types';

  let {
    agents,
    value = null,
    disabled = false,
    onChange,
  }: {
    agents: Agent[];
    value?: string | null;
    disabled?: boolean;
    onChange: (agentId: string | null) => void;
  } = $props();

  function handleChange(e: Event) {
    const v = (e.target as HTMLSelectElement).value;
    onChange(v === '' ? null : v);
  }
</script>

<div class="px-3 py-2">
  <label for="agent-select" class="block text-xs font-semibold uppercase tracking-wider text-stone-500 mb-1">Agent</label>
  <select id="agent-select"
    class="w-full rounded bg-stone-800 border border-stone-700 text-stone-200 text-sm px-2 py-1.5 disabled:opacity-50"
    value={value ?? ''}
    {disabled}
    onchange={handleChange}
  >
    <option value="">— none —</option>
    {#each agents as agent (agent.name)}
      <option value={agent.name}>{agent.name}</option>
    {/each}
  </select>
</div>
