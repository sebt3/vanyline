<script setup lang="ts">
import { computed, inject, nextTick, onMounted, onUnmounted, ref, watch } from 'vue';
import { useChat } from '@ai-sdk/vue';
import type { ChatTransport, UIMessage, ChatBackend } from '../ports';
import { Markdown } from '@comark/vue';

const props = defineProps<{ conversationId: string }>();

const chatBackend = inject<ChatBackend>('vanyline.chatBackend');
const transport = inject<ChatTransport<UIMessage>>('vanyline.chatTransport');
const notifyFsChange = inject<() => void>('notify-fs-change', () => {});

const { messages, status, sendMessage, error, stop } = useChat({
  id: props.conversationId,
  transport,
});

const displayMessages = computed(() =>
  messages.value.map((m) => ({
    ...m,
    parts: (m.parts as unknown[]).map((p) => ({ ...(p as Record<string, unknown>) })) as unknown as UIMessage['parts'],
  })),
);

const chatMessagesRef = ref<HTMLElement | null>(null);

watch(
  displayMessages,
  async () => {
    if (status.value !== 'streaming') return;
    await nextTick();
    const raw = chatMessagesRef.value as unknown as { $el?: HTMLElement } | HTMLElement | null;
    const el = (raw as { $el?: HTMLElement })?.$el ?? (raw as HTMLElement | null);
    const target = el ?? (document.querySelector('.chat-messages') as HTMLElement | null);
    if (!target) return;
    const scrollParent = target.parentElement?.classList.contains('chat-messages')
      ? target.parentElement
      : target;
    const scrollEl = (scrollParent.scrollHeight ? scrollParent : target) as HTMLElement;
    scrollEl.scrollTop = scrollEl.scrollHeight;
  },
  { deep: true },
);

const FS_MUTATING_TOOLS = new Set(['write_file', 'edit_file', 'delete_file', 'execute_command', 'edit_and_check']);
const seenToolResults = new Set<string>();
watch(
  messages,
  (msgs) => {
    for (const m of msgs) {
      for (const p of m.parts as Array<Record<string, unknown>>) {
        if (p.type !== 'dynamic-tool') continue;
        if (p.state !== 'output-available') continue;
        const toolCallId = p.toolCallId as string | undefined;
        if (toolCallId && seenToolResults.has(toolCallId)) continue;
        const toolName = p.toolName as string | undefined;
        if (!toolName || !FS_MUTATING_TOOLS.has(toolName)) continue;
        if (toolCallId) seenToolResults.add(toolCallId);
        notifyFsChange();
      }
    }
  },
  { deep: true },
);

onUnmounted(() => {
  void stop();
});

onMounted(async () => {
  try {
    const records = await chatBackend!.loadMessages(props.conversationId);
    messages.value = records.map((r) => ({ id: r.id, role: r.role, parts: [{ type: 'text', text: r.content }] })) as UIMessage[];
  } catch {
    // Historique optionnel — une conversation neuve n'a simplement aucun message.
  }
});

const input = ref('');

function onSubmit(e: Event) {
  e.preventDefault();
  const text = input.value.trim();
  if (!text || status.value === 'streaming' || status.value === 'submitted') return;
  input.value = '';
  void sendMessage({ text });
}
</script>

<template>
  <div class="chat-session">
    <Suspense>
      <UChatMessages
        ref="chatMessagesRef"
        :messages="displayMessages"
        :status="status"
        should-auto-scroll
        class="chat-messages"
      >
        <template #content="{ parts }">
          <template v-for="(part, idx) in parts" :key="idx">
            <UChatReasoning
              v-if="part.type === 'reasoning'"
              :text="part.text"
              :streaming="part.state === 'streaming'"
            />
            <Markdown v-else-if="part.type === 'text'" :value="part.text" streaming class="chat-markdown" />
            <UChatTool
              v-else-if="part.type === 'dynamic-tool'"
              :text="part.toolName"
              :loading="part.state === 'input-streaming' || part.state === 'input-available'"
            />
            <div v-else-if="part.type === 'data-tool_unavailable'" class="tool-unavailable">
              ⚠️ Outils indisponibles ({{ (part as { data: { server: string } }).data.server }}) :
              {{ (part as { data: { reason: string } }).data.reason }}
            </div>
          </template>
        </template>
      </UChatMessages>
    </Suspense>
    <div v-if="error" class="chat-error">⚠️ {{ error.message }}</div>
    <UChatPrompt v-model="input" class="chat-prompt" @submit="onSubmit">
      <UChatPromptSubmit :status="status" />
    </UChatPrompt>
  </div>
</template>

<style scoped>
.chat-session {
  height: 100%;
  display: flex;
  flex-direction: column;
  min-height: 0;
}

.chat-messages {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
}

.chat-prompt {
  flex: none;
}

.chat-error {
  flex: none;
  padding: 6px 12px;
  font-size: 12px;
  color: #e85d5d;
}

.tool-unavailable {
  font-size: 12px;
  color: #d9a441;
  padding: 4px 0;
}
</style>