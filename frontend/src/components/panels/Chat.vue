<script setup lang="ts">
import { inject, onMounted, ref, watch } from 'vue';
import { createApiClient } from '../../api/client';
import { endAgentSession, startAgentSession, useIdeSession } from '../../composables/useIdeSession';
import ChatSession from './ChatSession.vue';

/** Ligne renvoyée par `GET /api/conversations` — alimente le sélecteur de
 *  session (reprendre une conversation passée plutôt que d'en recréer une).
 *  Type local : le backend renvoie `id` en i32, on le convertit en `string`
 *  au chargement (`loadConversations`) pour garder `activeConversationId`,
 *  le sélecteur et les URLs WS typés string. */
interface ConversationOut {
  id: string;
  title: string | null;
  created_at: string;
}

// Posé par `IdeShell.vue` (route `/p/:project/s/:sandbox`) — même pattern
// que `Explorer.vue`. Nécessaire pour scoper la création et la liste des
// conversations à la sandbox courante (docs/features/chat-app-fonctionnel.md,
// axe 1 : le contexte est ce qui permet au backend de résoudre les tools MCP
// de cette sandbox pour le tour).
const sandboxName = inject<string>('sandbox-name', '');

const { activeConversationId, startingSession } = useIdeSession();
const client = createApiClient();

const conversations = ref<ConversationOut[]>([]);

async function loadConversations() {
  try {
    const query = sandboxName ? `?sandbox_name=${encodeURIComponent(sandboxName)}` : '';
    const rows = await client.get<ConversationOut[]>(`/api/conversations${query}`);
    conversations.value = rows.map((c) => ({ ...c, id: String(c.id) }));
  } catch {
    // Liste optionnelle pour le sélecteur — un échec ne doit pas bloquer
    // la session déjà active.
  }
}

function conversationLabel(c: ConversationOut): string {
  return c.title ?? `Session du ${new Date(c.created_at).toLocaleString('fr-FR')}`;
}

function onSelectConversation(event: Event) {
  const id = (event.target as HTMLSelectElement).value;
  if (id && id !== activeConversationId.value) {
    activeConversationId.value = id;
  }
}

watch(activeConversationId, () => {
  void loadConversations();
});

onMounted(() => {
  void loadConversations();
});
</script>

<template>
  <div class="chat-host">
    <div class="session-bar">
      <select
        class="session-select"
        aria-label="Session"
        :value="activeConversationId ?? ''"
        @change="onSelectConversation"
      >
        <option v-for="c in conversations" :key="c.id" :value="c.id">
          {{ conversationLabel(c) }}
        </option>
      </select>
      <button
        class="session-btn"
        :disabled="startingSession"
        title="Nouvelle session"
        @click="startAgentSession(sandboxName)"
      >
        +
      </button>
      <button class="session-btn" title="Fermer la session" @click="endAgentSession()">
        ×
      </button>
    </div>
    <ChatSession
      v-if="activeConversationId"
      :key="activeConversationId"
      :conversation-id="activeConversationId"
      class="chat-session-slot"
    />
  </div>
</template>

<style scoped>
.chat-host {
  height: 100%;
  display: flex;
  flex-direction: column;
  background: var(--dv-group-view-background-color);
}

.chat-session-slot {
  flex: 1;
  min-height: 0;
}

.session-bar {
  display: flex;
  align-items: center;
  gap: 4px;
  height: 34px;
  padding: 0 6px;
  background: var(--dv-color-abyss-light);
  border-bottom: 1px solid var(--dv-color-abyss-lighter);
}

.session-select {
  flex: 1;
  min-width: 0;
  height: 22px;
  background: var(--dv-color-abyss-lighter);
  color: var(--dv-color-abyss-primary-text);
  border: 1px solid #2b2b4a;
  border-radius: 4px;
  font-size: 11.5px;
  padding: 0 4px;
}

.session-btn {
  flex: none;
  width: 22px;
  height: 22px;
  background: var(--dv-color-abyss-lighter);
  color: var(--dv-color-abyss-primary-text);
  border: 1px solid #2b2b4a;
  border-radius: 4px;
  font-size: 13px;
  line-height: 1;
  cursor: pointer;
}

.session-btn:hover {
  background: #2b2b4a;
}

.session-btn:disabled {
  color: var(--dv-color-abyss-secondary-text);
  cursor: not-allowed;
}
</style>
