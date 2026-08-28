<script setup lang="ts">
import { inject, onMounted, onUnmounted, ref, watch } from 'vue';
import { useChat } from '@ai-sdk/vue';
import type { UIMessage } from 'ai';
import { Markdown } from '@comark/vue';
import { VanylineChatTransport } from '../../api/chatTransport';
import { createApiClient } from '../../api/client';

const props = defineProps<{ conversationId: string }>();

/** Ligne persistée (`GET /conversations/{id}/messages`) — `payload` est un
 *  JSON libre côté backend (`vanyline_app::ws::chat::persist_message`),
 *  seul `content` nous intéresse pour reconstruire l'historique en `UIMessage`.
 *  Type local : le backend renvoie `id` en i32, on le convertit en `string`
 *  au chargement (l'id `UIMessage` est une string côté AI SDK). */
interface MessageRow {
  id: string;
  role: string;
  payload: { content?: string };
  created_at: string;
}

const client = createApiClient();
const transport = new VanylineChatTransport();

const notifyFsChange = inject<() => void>('notify-fs-change', () => {});

const { messages, status, sendMessage, error, stop } = useChat({
  id: props.conversationId,
  transport,
});

const FS_MUTATING_TOOLS = new Set(['write_file', 'edit_file', 'delete_file', 'execute_command']);
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

// `Chat` (AI SDK) n'a pas de nettoyage automatique à l'unmount — sans ça,
// fermer la session ou changer de conversation en plein streaming laisse le
// WS ouvert côté navigateur jusqu'au `done`/`error` naturel du tour (le
// composant qui le lisait n'existe plus). `stop()` avorte le flux, ce qui
// déclenche `abortSignal` côté `VanylineChatTransport` et ferme le WS.
onUnmounted(() => {
  void stop();
});

onMounted(async () => {
  try {
    const rows = await client.get<MessageRow[]>(`/api/conversations/${props.conversationId}/messages`);
    messages.value = rows.map((m) => ({
      id: String(m.id),
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
      <UChatMessages
        :messages="messages"
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
