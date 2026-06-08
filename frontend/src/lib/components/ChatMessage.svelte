<script lang="ts">
  export interface UiMessage {
    id?: string;
    role: 'user' | 'assistant';
    content: string;
    streaming?: boolean;
    tool_calls?: Array<{ name: string; args: unknown }>;
  }

  let { message }: { message: UiMessage } = $props();
</script>

<div class="flex {message.role === 'user' ? 'justify-end' : 'justify-start'} px-4 py-1">
  <div
    class="max-w-prose rounded-lg px-4 py-2 text-sm {message.role === 'user'
      ? 'bg-primary-700 text-white'
      : 'bg-stone-800 text-stone-100'}"
  >
    {#if message.tool_calls && message.tool_calls.length > 0}
      {#each message.tool_calls as tc}
        <div class="text-xs text-stone-400 font-mono mb-1">
          <span class="text-primary-400">⚡ {tc.name}</span>
          <span class="text-stone-500"> {JSON.stringify(tc.args)}</span>
        </div>
      {/each}
    {/if}
    <span class="whitespace-pre-wrap">{message.content}</span>
    {#if message.streaming}
      <span class="inline-block w-2 h-4 bg-primary-400 animate-pulse ml-0.5 align-middle"></span>
    {/if}
  </div>
</div>
