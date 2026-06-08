<script lang="ts">
  import type { Conversation } from '$lib/types';

  let {
    conversations,
    activeId = null,
    loading = false,
    onSelect,
    onDelete,
    onNew,
  }: {
    conversations: Conversation[];
    activeId?: string | null;
    loading?: boolean;
    onSelect: (id: string) => void;
    onDelete: (id: string) => void;
    onNew: () => void;
  } = $props();

  function formatDate(iso: string): string {
    return new Date(iso).toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
  }
</script>

<div class="flex flex-col h-full">
  <div class="flex items-center justify-between px-3 py-2 border-b border-stone-800">
    <span class="text-xs font-semibold uppercase tracking-wider text-stone-500">Conversations</span>
    <button
      onclick={onNew}
      class="text-stone-400 hover:text-stone-100 text-lg leading-none px-1"
      title="New conversation"
    >+</button>
  </div>

  {#if loading}
    <div class="flex-1 flex items-center justify-center">
      <span class="text-stone-500 text-sm">Loading…</span>
    </div>
  {:else if conversations.length === 0}
    <div class="flex-1 flex items-center justify-center px-4">
      <p class="text-stone-600 text-sm text-center">No conversations yet.<br />Click + to start one.</p>
    </div>
  {:else}
    <ul class="flex-1 overflow-y-auto py-1">
      {#each conversations as conv (conv.id)}
        <li class="group flex items-center gap-1 px-2 py-1">
          <button
            onclick={() => onSelect(conv.id)}
            class="flex-1 text-left truncate rounded px-2 py-1.5 text-sm {conv.id === activeId
              ? 'bg-primary-800 text-white'
              : 'text-stone-300 hover:bg-stone-800'}"
          >
            <span class="truncate block">{conv.title ?? 'New conversation'}</span>
            <span class="text-xs text-stone-500">{formatDate(conv.created_at)}</span>
          </button>
          <button
            onclick={() => onDelete(conv.id)}
            class="opacity-0 group-hover:opacity-100 text-stone-600 hover:text-red-400 text-xs px-1"
            title="Delete"
          >✕</button>
        </li>
      {/each}
    </ul>
  {/if}
</div>
