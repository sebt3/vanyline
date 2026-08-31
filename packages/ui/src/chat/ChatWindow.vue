<script setup lang="ts">
import { inject, onMounted, ref, watch } from 'vue';
import type { ChatBackend, ChatConversation } from '../ports';
import ChatSession from './ChatSession.vue';

interface Props {
  activeConversationId: string | null; // v-model — l'embarqueur le possède
  startingSession?: boolean;           // défaut false — état global, l'embarqueur le possède
}
type Emits = { 'update:activeConversationId': [string | null] };

const props = withDefaults(defineProps<Props>(), {
  startingSession: false,
});
const emit = defineEmits<Emits>();

const chatBackend = inject<ChatBackend>('vanyline.chatBackend');
const conversations = ref<ChatConversation[]>([]);

async function loadConversations() {
  try {
    conversations.value = await chatBackend!.listConversations();
  } catch {
    // Liste optionnelle pour le sélecteur — un échec ne bloque pas
    // la session déjà active.
  }
}

function conversationLabel(c: ChatConversation): string {
  return c.title ?? `Session du ${new Date(c.createdAt).toLocaleString('fr-FR')}`;
}

function onSelectConversation(event: Event) {
  const id = (event.target as HTMLSelectElement).value;
  if (id && id !== props.activeConversationId) {
    emit('update:activeConversationId', id);
  }
}

watch(() => props.activeConversationId, () => {
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
        :disabled="props.startingSession"
        title="Nouvelle session"
        @click="void chatBackend!.createConversation().then((id) => emit('update:activeConversationId', id)).catch(() => {})"
      >
        +
      </button>
      <button class="session-btn" title="Fermer la session" @click="emit('update:activeConversationId', null)">
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