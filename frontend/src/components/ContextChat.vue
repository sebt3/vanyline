<script setup lang="ts">
import { computed, onMounted, provide, ref, watch } from 'vue';
import { ChatWindow } from '@vanyline/ui';
import { VanylineChatTransport } from '../api/chatTransport';
import {
  contextChatBackend,
  contextChatErrorMessage,
  createContextConversation,
  findOrCreateContextConversation,
  isNoModelProfileError,
  isNoModelProfileMessage,
  listAgents,
  listModelProfiles,
  observeTurns,
  setConfigAgentModel,
  systemAgentName,
  type AgentRow,
  type ContextData,
  type ContextKind,
  type ModelProfileRow,
} from '../api/contextChatBackend';

const props = withDefaults(defineProps<{ kind: ContextKind; data?: ContextData }>(), {
  data: () => ({}),
});
const emit = defineEmits<{ 'turn-finished': [] }>();

type Phase = 'loading' | 'ready' | 'no-model-profile' | 'error';

const phase = ref<Phase>('loading');
const errorMessage = ref<string | null>(null);
const activeConversationId = ref<string | null>(null);
const selectedAgent = ref('');
const ordinaryAgents = ref<AgentRow[]>([]);
const configAgent = ref<AgentRow | null>(null);
const profiles = ref<ModelProfileRow[]>([]);

const isSettings = computed(() => props.kind === 'settings');
const defaultAgent = computed(() => systemAgentName(props.kind));

const backend = contextChatBackend({
  kind: () => props.kind,
  data: () => props.data,
  agent: () => selectedAgent.value || defaultAgent.value,
});

provide('vanyline.chatBackend', backend);
provide(
  'vanyline.chatTransport',
  observeTurns(new VanylineChatTransport(), {
    onTurnFinished: () => emit('turn-finished'),
    onTurnError: (message) => {
      if (isNoModelProfileMessage(message)) phase.value = 'no-model-profile';
    },
  }),
);

/** Agents et profils : listes d'aide. Un échec ne bloque pas le chat ; le défaut reste utilisable. */
async function loadContextLists() {
  try {
    const agents = await listAgents();
    ordinaryAgents.value = agents.filter((a) => a.system_key === null);
    configAgent.value = agents.find((a) => a.system_key === 'config') ?? null;
  } catch {
    // Liste optionnelle : le sélecteur ne propose que le défaut.
  }
  if (isSettings.value) {
    try {
      profiles.value = await listModelProfiles();
    } catch {
      // Sans liste, le sélecteur de modèle reste vide.
    }
  }
}

async function init() {
  phase.value = 'loading';
  errorMessage.value = null;
  activeConversationId.value = null;
  try {
    const conv = await findOrCreateContextConversation(props.kind, props.data, defaultAgent.value);
    activeConversationId.value = String(conv.id);
    selectedAgent.value = conv.agent_name ?? defaultAgent.value;
    await loadContextLists();
    phase.value = 'ready';
  } catch (err) {
    if (isNoModelProfileError(err)) {
      phase.value = 'no-model-profile';
    } else {
      phase.value = 'error';
      errorMessage.value = contextChatErrorMessage(err);
    }
  }
}

async function onAgentChange(event: Event) {
  const name = (event.target as HTMLSelectElement).value;
  if (name === selectedAgent.value) return;
  errorMessage.value = null;
  try {
    // L'agent d'une conversation est fixé à sa création : changer d'agent ouvre une
    // nouvelle conversation du même contexte avec l'agent choisi.
    const conv = await createContextConversation(props.kind, props.data, name);
    selectedAgent.value = conv.agent_name ?? name;
    activeConversationId.value = String(conv.id);
  } catch (err) {
    if (isNoModelProfileError(err)) {
      phase.value = 'no-model-profile';
    } else {
      errorMessage.value = contextChatErrorMessage(err);
    }
  }
}

async function onProfileChange(event: Event) {
  const agent = configAgent.value;
  const profileId = Number((event.target as HTMLSelectElement).value);
  if (!agent || profileId === agent.model_profile_id) return;
  errorMessage.value = null;
  try {
    configAgent.value = await setConfigAgentModel(agent, profileId);
  } catch (err) {
    errorMessage.value = contextChatErrorMessage(err);
  }
}

onMounted(() => {
  void init();
});

watch(
  () => [props.kind, JSON.stringify(props.data)],
  () => {
    void init();
  },
);
</script>

<template>
  <div class="context-chat">
    <div v-if="phase === 'loading'" class="context-chat__status" role="status">
      Chargement de l’assistant…
    </div>

    <div v-else-if="phase === 'no-model-profile'" class="context-chat__no-profile" role="alert">
      <p class="context-chat__no-profile-title">Aucun profil de modèle</p>
      <p>
        L’assistant a besoin d’un profil de modèle pour répondre. Crée d’abord un fournisseur
        et un profil de modèle dans les réglages, puis reviens ici.
      </p>
    </div>

    <div v-else-if="phase === 'error'" class="context-chat__status context-chat__status--error" role="alert">
      {{ errorMessage }}
    </div>

    <template v-else>
      <header class="context-chat__header">
        <label v-if="isSettings && configAgent" class="context-chat__field">
          <span>Modèle</span>
          <select
            aria-label="Modèle de l'assistant"
            :value="configAgent.model_profile_id"
            @change="onProfileChange"
          >
            <option v-for="p in profiles" :key="p.id" :value="p.id">{{ p.name }}</option>
          </select>
        </label>
        <label v-else-if="!isSettings" class="context-chat__field">
          <span>Agent</span>
          <select aria-label="Agent" :value="selectedAgent" @change="onAgentChange">
            <option :value="defaultAgent">Agent du contexte (défaut)</option>
            <option v-for="a in ordinaryAgents" :key="a.id" :value="a.name">{{ a.name }}</option>
          </select>
        </label>
      </header>

      <p v-if="errorMessage" class="context-chat__error" role="alert">{{ errorMessage }}</p>

      <ChatWindow v-model:activeConversationId="activeConversationId" class="context-chat__chat" />
    </template>
  </div>
</template>

<style scoped>
/* Occupe le reste de la colonne parente (sous la barre d'en-tête) : jamais height 100%. */
.context-chat {
  display: flex;
  flex-direction: column;
  flex: 1;
  width: 100%;
  min-width: 0;
  min-height: 0;
  overflow-x: hidden;
}

.context-chat__status,
.context-chat__no-profile,
.context-chat__error {
  padding: 12px 16px;
  font-size: 13px;
}

.context-chat__no-profile-title {
  font-weight: 600;
}

.context-chat__header {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  padding: 8px 12px;
}

.context-chat__field {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  min-width: 0;
}

.context-chat__field select {
  min-height: var(--ctl-h);
  max-width: 100%;
  min-width: 0;
}

.context-chat__chat {
  flex: 1;
  min-height: 0;
}
</style>
