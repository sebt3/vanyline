<script setup lang="ts">
import { provide, ref } from 'vue';
import { ChatWindow } from '@vanyline/ui';

// Spike (tâche 01) : ports stub — le transport réel postMessage arrive en tâche 04.
provide('vanyline.chatBackend', {
  async listConversations() { return [] as import('@vanyline/ui').ChatConversation[]; },
  async loadMessages() { return [] as import('@vanyline/ui').ChatMessageRecord[]; },
  async createConversation() { return 'spike-session'; },
});
provide('vanyline.chatTransport', {
  async sendMessages() { return new ReadableStream({ start(c) { c.close(); } }); },
});

// null → ChatSession (Nuxt UI UChat*) n'est jamais monté ici : le spike prouve
// la webview via ChatWindow seul.
const activeConversationId = ref<string | null>(null);
</script>

<template>
  <ChatWindow v-model:activeConversationId="activeConversationId" />
</template>
