<script setup lang="ts">
import { onMounted, ref, useTemplateRef } from 'vue';

interface ChatMessage {
  _id: string;
  senderId: string;
  content: string;
  username?: string;
  timestamp?: string;
  saved?: boolean;
  distributed?: boolean;
  seen?: boolean;
}

const rooms = [
  {
    roomId: 'assistant',
    roomName: 'Assistant',
    avatar: '',
    users: [
      { _id: 'me', username: 'toi', status: { state: 'online', lastChanged: '' } },
      { _id: 'assistant', username: 'Assistant', status: { state: 'online', lastChanged: '' } },
    ],
  },
];

const initialMessages: ChatMessage[] = [
  {
    _id: '1',
    senderId: 'me',
    content: 'Relance la synchro de la bibliothèque depuis le NAS, le dossier "Documentaires" a bougé.',
    timestamp: '10:41',
    saved: true,
    distributed: true,
    seen: true,
  },
  {
    _id: '2',
    senderId: 'assistant',
    username: 'Assistant',
    content:
      'Le DAG sync-media est lancé. fetch-metadata et transcode sont terminés, generate-thumbnails est en cours (~40 fichiers). Je te préviens si extract-subtitles échoue.',
    timestamp: '10:41',
  },
  {
    _id: '3',
    senderId: 'me',
    content: 'Ok, montre-moi sync_library.py pendant que ça tourne.',
    timestamp: '10:42',
    saved: true,
    distributed: true,
    seen: true,
  },
];

// vue-advanced-chat attend une vraie transition false -> true pour lever
// son spinner de chargement, pas juste une prop toujours à `true` (voir
// son README, section "Follow the UI loading pattern") : sans ça, il
// reste bloqué dans son état de chargement initial.
const messages = ref<ChatMessage[]>([]);
const messagesLoaded = ref(false);
const roomsLoaded = ref(false);

const chatEl = useTemplateRef<HTMLElement>('chatEl');

// Le prop `theme` de la lib pose ses couleurs via element.style.setProperty(...)
// sur cet hôte — du inline, potentiellement avec sa propre priorité `important`,
// que même une règle externe !important ne bat pas de façon fiable (constaté :
// aucun effet visuel malgré le !important CSS ci-dessous). Seule certitude : la
// dernière écriture sur le même style inline gagne. On réapplique donc nous-mêmes
// en JS, après le montage — deux passages pour couvrir une éventuelle
// (ré)application asynchrone de la lib après notre premier essai.
const chatVars: Record<string, string> = {
  '--chat-container-border': 'none',
  '--chat-container-border-radius': '0',
  '--chat-container-box-shadow': 'none',
  '--chat-content-bg-color': '#00141e',
  '--chat-header-bg-color': '#1c1c2a',
  '--chat-header-color-name': '#ffffff',
  '--chat-header-color-info': '#8a96a6',
  '--chat-footer-bg-color': '#1c1c2a',
  '--chat-footer-bg-color-reply': '#2b2b4a',
  '--chat-bg-color-input': '#2b2b4a',
  '--chat-color': '#ffffff',
  '--chat-color-placeholder': '#8a96a6',
  '--chat-color-caret': '#ffffff',
  '--chat-message-bg-color': '#1c1c2a',
  '--chat-message-bg-color-me': '#2c3037',
  '--chat-message-color': '#ffffff',
  '--chat-message-color-timestamp': '#8a96a6',
  '--chat-message-color-username': '#8a96a6',
  '--chat-icon-color-send': '#5b1ecf',
  '--chat-icon-color-emoji': '#8a96a6',
  '--chat-icon-color-paperclip': '#8a96a6',
  '--chat-color-spinner': '#8a96a6',
};

function applyChatTheme() {
  const el = chatEl.value;
  if (!el) return;
  for (const [name, value] of Object.entries(chatVars)) {
    el.style.setProperty(name, value, 'important');
  }
}

onMounted(() => {
  applyChatTheme();
  setTimeout(() => {
    messages.value = initialMessages;
    messagesLoaded.value = true;
    roomsLoaded.value = true;
    applyChatTheme();
  });
  setTimeout(applyChatTheme, 300);
});

function onSendMessage(event: Event) {
  const detail = (event as CustomEvent).detail;
  const payload = Array.isArray(detail) ? detail[0] : detail;
  const content: string = payload?.content ?? '';
  if (!content.trim()) return;

  messages.value = [
    ...messages.value,
    {
      _id: String(messages.value.length + 1),
      senderId: 'me',
      content,
      timestamp: new Date().toLocaleTimeString('fr-FR', { hour: '2-digit', minute: '2-digit' }),
      saved: true,
      distributed: true,
      seen: true,
    },
  ];
}
</script>

<template>
  <div class="chat-host">
    <vue-advanced-chat
      ref="chatEl"
      height="100%"
      theme="dark"
      :current-user-id="'me'"
      :rooms="JSON.stringify(rooms)"
      :rooms-loaded="roomsLoaded"
      :messages="JSON.stringify(messages)"
      :messages-loaded="messagesLoaded"
      :single-room="true"
      :show-audio="false"
      :show-files="false"
      :room-info-enabled="false"
      @send-message="onSendMessage"
    />
  </div>
</template>

<style scoped>
.chat-host {
  height: 100%;
  background: var(--dv-group-view-background-color);
}

/*
 * vue-advanced-chat n'attend pas ces custom properties passivement : son
 * runtime fait lui-même du element.style.setProperty(...) sur l'hôte en
 * réaction au prop `theme`, donc du style *inline* posé par la lib après
 * coup. Une règle externe classique perd toujours face à de l'inline —
 * il faut du !important pour gagner le cascade depuis l'extérieur.
 */
.chat-host :deep(vue-advanced-chat) {
  --chat-container-border: none !important;
  --chat-container-border-radius: 0 !important;
  --chat-container-box-shadow: none !important;
  --font-family: -apple-system, "Segoe UI", system-ui, sans-serif !important;

  /* Alignée sur la couleur réellement rendue par l'Explorer (#00141e). */
  --chat-content-bg-color: #00141e !important;
  --chat-header-bg-color: var(--dv-color-abyss-light) !important;
  --chat-header-color-name: var(--dv-color-abyss-primary-text) !important;
  --chat-header-color-info: var(--dv-color-abyss-secondary-text) !important;

  --chat-footer-bg-color: var(--dv-color-abyss-light) !important;
  --chat-bg-color-input: var(--dv-color-abyss-lighter) !important;
  --chat-color: var(--dv-color-abyss-primary-text) !important;
  --chat-color-placeholder: var(--dv-color-abyss-secondary-text) !important;
  --chat-color-caret: var(--dv-color-abyss-primary-text) !important;

  --chat-message-bg-color: var(--dv-color-abyss-light) !important;
  --chat-message-bg-color-me: #2c3037 !important;
  --chat-message-color: var(--dv-color-abyss-primary-text) !important;
  --chat-message-color-timestamp: var(--dv-color-abyss-secondary-text) !important;
  --chat-message-color-username: var(--dv-color-abyss-secondary-text) !important;

  --chat-icon-color-send: #5b1ecf !important;
  --chat-icon-color-emoji: var(--dv-color-abyss-secondary-text) !important;
  --chat-icon-color-paperclip: var(--dv-color-abyss-secondary-text) !important;
  --chat-color-spinner: var(--dv-color-abyss-secondary-text) !important;
}
</style>
