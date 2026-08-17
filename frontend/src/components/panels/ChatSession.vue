<script setup lang="ts">
import { onMounted, ref } from 'vue';
import { useChat } from '@ai-sdk/vue';
import type { UIMessage } from 'ai';
import { Markdown } from '@comark/vue';
import { VanylineChatTransport } from '../../api/chatTransport';
import { createApiClient } from '../../api/client';

const props = defineProps<{ conversationId: string }>();

/** Ligne persistée (`GET /conversations/{id}/messages`) — `payload` est un
 *  JSON libre côté backend (`vanyline_app::ws::chat::persist_message`),
 *  seul `content` nous intéresse pour reconstruire l'historique en `UIMessage`. */
interface MessageRow {
  id: string;
  role: string;
  payload: { content?: string };
  created_at: string;
}

const client = createApiClient();
const transport = new VanylineChatTransport();

const { messages, status, sendMessage, error } = useChat({
  id: props.conversationId,
  transport,
});

onMounted(async () => {
  try {
    const rows = await client.get<MessageRow[]>(`/api/conversations/${props.conversationId}/messages`);
    messages.value = rows.map((m) => ({
      id: m.id,
      role: m.role === 'user' ? 'user' : 'assistant',
      parts: [{ type: 'text', text: m.payload.content ?? '' }],
    })) as UIMessage[];
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
    <!-- `Markdown` (@comark/vue) a un setup() async — Suspense obligatoire,
         sinon Vue avertit et ne monte pas le composant (cf. warning
         "component with async setup must be nested in a Suspense"). -->
    <Suspense>
      <UChatMessages :messages="messages" :status="status" class="chat-messages">
        <template #content="{ parts }">
          <template v-for="(part, idx) in parts" :key="idx">
            <Markdown v-if="part.type === 'text'" :value="part.text" streaming class="chat-markdown" />
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
