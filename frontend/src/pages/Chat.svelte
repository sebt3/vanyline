<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { push } from 'svelte-spa-router';

  import { conversationsStore } from '$lib/stores/conversations.svelte';
  import { agentsStore } from '$lib/stores/agents.svelte';
  import { userStore } from '$lib/stores/user.svelte';
  import { getMessages } from '$lib/api/conversations';
  import { wsUrl } from '$lib/api/client';

  import ConversationList from '$lib/components/ConversationList.svelte';
  import AgentSelector from '$lib/components/AgentSelector.svelte';
  import ChatMessage, { type UiMessage } from '$lib/components/ChatMessage.svelte';
  import ChatInput from '$lib/components/ChatInput.svelte';

  import type { WsServerMessage } from '$lib/types';

  let { params = {} }: { params?: { id?: string } } = $props();

  let conversationId = $derived(params?.id ?? null);
  let selectedAgentId = $state<string | null>(null);

  let messages = $state<UiMessage[]>([]);
  let streaming = $state(false);
  let streamingMsg = $state<UiMessage | null>(null);

  let ws: WebSocket | null = null;
  let messagesEnd = $state<HTMLDivElement | undefined>(undefined);

  $effect(() => {
    if (!userStore.loading && !userStore.email) {
      push('/login');
    }
  });

  onMount(async () => {
    await Promise.all([conversationsStore.load(), agentsStore.load()]);
  });

  $effect(() => {
    if (conversationId) {
      conversationsStore.setActive(conversationId);
      loadConversationMessages(conversationId);
      reconnectWs(conversationId);
    } else {
      conversationsStore.setActive(null);
      messages = [];
      closeWs();
    }
  });

  onDestroy(() => closeWs());

  async function loadConversationMessages(id: string) {
    messages = [];
    streaming = false;
    streamingMsg = null;
    try {
      const raw = await getMessages(id);
      messages = raw.map((m) => ({
        id: m.id,
        role: m.role as 'user' | 'assistant',
        content: typeof m.payload.content === 'string' ? m.payload.content : '',
      }));
    } catch {
      messages = [];
    }
    scrollToBottom();
  }

  function reconnectWs(id: string) {
    closeWs();
    ws = new WebSocket(wsUrl(`/api/ws/chat/${id}`));

    ws.onmessage = (event) => {
      const msg: WsServerMessage = JSON.parse(event.data);
      handleServerMessage(msg);
    };

    ws.onclose = () => {
      streaming = false;
      streamingMsg = null;
    };

    ws.onerror = () => {
      streaming = false;
      streamingMsg = null;
    };
  }

  function closeWs() {
    if (ws) {
      ws.close();
      ws = null;
    }
  }

  function handleServerMessage(msg: WsServerMessage) {
    if (msg.type === 'token') {
      if (!streamingMsg) {
        streamingMsg = { role: 'assistant', content: msg.content, streaming: true };
      } else {
        streamingMsg = { ...streamingMsg, content: streamingMsg.content + msg.content };
      }
      scrollToBottom();
    } else if (msg.type === 'tool_call') {
      if (streamingMsg) {
        const existing = streamingMsg.tool_calls ?? [];
        streamingMsg = { ...streamingMsg, tool_calls: [...existing, { name: msg.name, args: msg.args }] };
      }
    } else if (msg.type === 'done') {
      if (streamingMsg) {
        messages = [...messages, { ...streamingMsg, id: msg.message_id, streaming: false }];
        streamingMsg = null;
      }
      streaming = false;
      scrollToBottom();
    } else if (msg.type === 'error') {
      streaming = false;
      streamingMsg = null;
      messages = [...messages, { role: 'assistant', content: `Error: ${msg.message}` }];
      scrollToBottom();
    }
  }

  async function handleSend(content: string) {
    if (!conversationId || !ws || ws.readyState !== WebSocket.OPEN || streaming) return;

    messages = [...messages, { role: 'user', content }];
    streaming = true;
    streamingMsg = null;
    scrollToBottom();

    ws.send(JSON.stringify({ type: 'message', content }));
  }

  async function handleNewConversation() {
    const conv = await conversationsStore.create(selectedAgentId ?? undefined);
    push(`/chat/${conv.id}`);
  }

  async function handleDeleteConversation(id: string) {
    await conversationsStore.remove(id);
    if (conversationId === id) {
      push('/chat');
    }
  }

  function handleSelectConversation(id: string) {
    push(`/chat/${id}`);
  }

  function scrollToBottom() {
    requestAnimationFrame(() => messagesEnd?.scrollIntoView({ behavior: 'smooth' }));
  }

  let displayMessages = $derived([
    ...messages,
    ...(streamingMsg ? [streamingMsg] : []),
  ]);
</script>

{#if userStore.loading}
  <div class="flex min-h-screen items-center justify-center">
    <p class="text-stone-400">Loading…</p>
  </div>
{:else if userStore.email}
  <div class="flex h-screen overflow-hidden">
    <!-- Sidebar -->
    <aside class="w-64 flex-shrink-0 flex flex-col bg-stone-900 border-r border-stone-800">
      <div class="px-3 py-3 border-b border-stone-800">
        <h1 class="text-lg font-semibold text-primary-400">vanyline</h1>
        <p class="text-xs text-stone-500">{userStore.email}</p>
      </div>

      <AgentSelector
        agents={agentsStore.agents}
        value={selectedAgentId}
        onChange={(id) => { selectedAgentId = id; }}
      />

      <div class="flex-1 overflow-hidden">
        <ConversationList
          conversations={conversationsStore.conversations}
          activeId={conversationsStore.activeId}
          loading={conversationsStore.loading}
          onSelect={handleSelectConversation}
          onDelete={handleDeleteConversation}
          onNew={handleNewConversation}
        />
      </div>

      <div class="px-3 py-2 border-t border-stone-800">
        <a href="/auth/logout" class="text-xs text-stone-500 hover:text-stone-300">Sign out</a>
      </div>
    </aside>

    <!-- Main area -->
    <main class="flex-1 flex flex-col min-w-0">
      {#if !conversationId}
        <div class="flex-1 flex flex-col items-center justify-center text-stone-600 gap-3">
          <p class="text-lg">Select or create a conversation</p>
          <button
            onclick={handleNewConversation}
            class="rounded bg-primary-700 hover:bg-primary-600 text-white px-5 py-2 text-sm"
          >New conversation</button>
        </div>
      {:else}
        <div class="flex-1 overflow-y-auto py-4">
          {#each displayMessages as msg, i (msg.id ?? `msg-${i}`)}
            <ChatMessage message={msg} />
          {/each}
          <div bind:this={messagesEnd}></div>
        </div>
        <ChatInput disabled={streaming} onSend={handleSend} />
      {/if}
    </main>
  </div>
{/if}
