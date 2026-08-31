<script setup lang="ts">
import { provide, inject } from 'vue';
import { ChatWindow } from '@vanyline/ui';
import { VanylineChatTransport } from '../../api/chatTransport';
import { httpChatBackend } from '../../api/httpChatBackend';
import { useIdeSession } from '../../composables/useIdeSession';

const sandboxName = inject<string>('sandbox-name', '');

provide('vanyline.chatBackend', httpChatBackend(sandboxName));
provide('vanyline.chatTransport', new VanylineChatTransport());

const { activeConversationId, startingSession } = useIdeSession();
</script>

<template>
  <ChatWindow
    v-model:activeConversationId="activeConversationId"
    :starting-session="startingSession"
  />
</template>