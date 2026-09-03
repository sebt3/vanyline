<script setup lang="ts">
import { onMounted, provide, ref } from 'vue';
import { ChatWindow } from '@vanyline/ui';
import { getBridgeClient } from './bridge';
import { createBridgeBackend } from './backend';
import { PostMessageChatTransport } from './postMessageChatTransport';

// getBridgeClient() appelé ICI (setup), jamais à l'import du module :
// acquireVsCodeApi() n'existe que dans la webview et une seule fois (cf. bridge.ts).
const bridge = getBridgeClient();

// '' = agent par défaut : champ omis au send (getAgent renvoie undefined sur '').
// (Le `?? undefined` du prototype ne couvrait pas '' — String non nullish ; la
// sémantique gelée « champ omis au send » prime, d'où `|| undefined`.)
const selectedAgent = ref<string>('');
const activeConversationId = ref<string | null>(null);

const transport = new PostMessageChatTransport(bridge, () => selectedAgent.value || undefined);
provide('vanyline.chatBackend', createBridgeBackend(bridge));
provide('vanyline.chatTransport', transport);

const agents = ref<string[]>([]);
const agentsUnavailable = ref(false);

onMounted(() => {
  bridge
    .request<{ name: string }[]>('config/agents', {})
    .then((list) => {
      agents.value = list.map((a) => a.name);
    })
    .catch(() => {
      // -021 (serveur non démarré) ou erreur RPC → UI dégradée, jamais bloquante.
      agentsUnavailable.value = true;
    });

  // Commandes host (tâche 04a) : nouvelle session et reprise via QuickPick.
  bridge.onMessage('session/new', (id) => {
    if (id) activeConversationId.value = id;
  });
  bridge.onMessage('session/pick', (id) => {
    activeConversationId.value = id;
  });
});
</script>

<template>
  <select
    v-model="selectedAgent"
    class="agent-select"
    data-testid="agent-select"
    :disabled="agentsUnavailable"
  >
    <option value="">Agent (par défaut)</option>
    <option v-for="name in agents" :key="name" :value="name">{{ name }}</option>
  </select>
  <!-- activeConversationId null → ChatSession (Nuxt UI UChat*) n'est monté qu'après
       session/new|pick ou sélection dans le sélecteur interne de ChatWindow. -->
  <ChatWindow v-model:activeConversationId="activeConversationId" />
</template>
