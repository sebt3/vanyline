<script setup lang="ts">
import { onBeforeUnmount, onMounted, provide, shallowRef, ref, watch } from 'vue';
import { DockviewVue, type DockviewReadyEvent, type VueComponent } from 'dockview-vue';
import Explorer from './panels/Explorer.vue';
import Editor from './panels/Editor.vue';
import Workflow from './panels/Workflow.vue';
import Chat from './panels/Chat.vue';
import Terminal from './panels/Terminal.vue';
import { openSandboxWs, SandboxFsClient } from '../api/sandboxWs';
import { debounce, loadLayout, saveLayout } from './ideLayoutPersistence';
import { clearIdeActions, registerIdeActions, useIdeSession } from '../composables/useIdeSession';

const props = defineProps<{ sandboxName: string }>();

// Client /ws/fs partagé Explorer/Editor — UNE instance par sandbox ouverte.
// shallowRef : le client contient un WebSocket brut qu'il ne faut pas rendre
// profondément réactif. null tant que le ticket/minage n'est pas résolu.
const fsClient = shallowRef<SandboxFsClient | null>(null);
// Fichier ouvert : état remonté d'Explorer vers IdeShell, transmis à Editor.
const openFilePath = ref<string | null>(null);

provide('sandbox-fs', fsClient);
provide('sandbox-name', props.sandboxName);
provide('open-file-path', openFilePath);
// Handler fourni à Explorer : remonter l'ouverture d'un fichier.
provide('open-file', (path: string) => {
  openFilePath.value = path;
});

const { activeConversationId, sessionError } = useIdeSession();

onMounted(() => {
  openSandboxWs(props.sandboxName, '/ws/fs')
    .then((ws) => {
      fsClient.value = new SandboxFsClient(ws);
    })
    .catch(() => {
      // Ticket/ingress indisponible (dépendance d'infra) : les panneaux
      // restent vides (fsClient === null), sans planter l'IDE.
      fsClient.value = null;
    });
});

// Démonté = route sandbox quittée : plus de handlers valides côté menu,
// plus de session active à afficher.
onBeforeUnmount(() => {
  clearIdeActions();
});

const components = {
  explorer: Explorer,
  editor: Editor,
  workflow: Workflow,
  chat: Chat,
  terminal: Terminal,
} as unknown as Record<string, VueComponent>;

function addDefaultPanels(api: DockviewReadyEvent['api']) {
  api.addPanel({
    id: 'explorer',
    component: 'explorer',
    title: 'Explorer',
    initialWidth: 230,
  });

  api.addPanel({
    id: 'editor',
    component: 'editor',
    title: 'sync_library.py',
    position: { referencePanel: 'explorer', direction: 'right' },
  });

  api.addPanel({
    id: 'workflow',
    component: 'workflow',
    title: 'sync-media.dag',
    position: { referencePanel: 'editor', direction: 'within' },
  });

  api.addPanel({
    id: 'terminal',
    component: 'terminal',
    title: 'Terminal',
    position: { referencePanel: 'editor', direction: 'below' },
    initialHeight: 170,
  });

  // Pas de panel 'chat' par défaut : la colonne assistant n'existe que si
  // une vraie session agent est démarrée (cf. useIdeSession, watcher
  // ci-dessous). addChatPanel() la pose le moment venu.
  api.getPanel('editor')?.api.setActive();
}

function addChatPanel(api: DockviewReadyEvent['api']) {
  if (api.getPanel('chat')) return;
  api.addPanel({
    id: 'chat',
    component: 'chat',
    title: 'Assistant',
    position: { referencePanel: 'editor', direction: 'right' },
    initialWidth: 330,
  });
}

/** Réactive l'Explorer s'il est déjà ouvert, le recrée sinon — un seul
 *  Explorer possible (contrairement au terminal), donc pas besoin d'id
 *  généré. */
function openExplorer(api: DockviewReadyEvent['api']) {
  const existing = api.getPanel('explorer');
  if (existing) {
    existing.api.setActive();
    return;
  }
  api.addPanel({
    id: 'explorer',
    component: 'explorer',
    title: 'Explorer',
    position: { referencePanel: 'editor', direction: 'left' },
    initialWidth: 230,
  });
}

/** 'terminal' pour le premier onglet (id stable, restauré par le layout
 *  persisté), 'terminal-N' pour les suivants — chaque panel Terminal.vue
 *  ouvre sa propre connexion /ws/terminal indépendante (pas d'état partagé
 *  entre onglets), donc aucun changement requis côté Terminal.vue. */
function addTerminalPanel(api: DockviewReadyEvent['api']) {
  let id = 'terminal';
  let n = 2;
  while (api.getPanel(id)) {
    id = `terminal-${n}`;
    n += 1;
  }
  api.addPanel({
    id,
    component: 'terminal',
    title: id === 'terminal' ? 'Terminal' : `Terminal ${n - 1}`,
    position: { referencePanel: 'editor', direction: 'below' },
    initialHeight: 170,
  });
  api.getPanel(id)?.api.setActive();
}

function onReady(event: DockviewReadyEvent) {
  const { api } = event;

  // Répartition sauvegardée pour cette sandbox : la restaurer plutôt que
  // rebâtir le layout par défaut. Un layout absent/corrompu retombe sur le
  // layout par défaut sans planter (cf. loadLayout).
  const saved = loadLayout(props.sandboxName);
  if (saved) {
    api.fromJSON(saved);
    // Une session ne survit pas au rechargement de page (activeConversationId
    // repart à null) — un panel 'chat' resté dans un layout sauvegardé ne
    // correspond donc plus à une session réelle : on le referme.
    api.getPanel('chat')?.api.close();
  } else {
    addDefaultPanels(api);
  }

  // onDidLayoutChange se déclenche à chaque frame d'un drag — anti-rebond
  // avant d'écrire dans localStorage.
  const persist = debounce(() => saveLayout(props.sandboxName, api.toJSON()), 400);
  api.onDidLayoutChange(persist);

  registerIdeActions({
    closeActiveTab: () => api.activePanel?.api.close(),
    openExplorer: () => openExplorer(api),
    newTerminal: () => addTerminalPanel(api),
  });

  watch(activeConversationId, (id) => {
    if (id) {
      addChatPanel(api);
      api.getPanel('chat')?.api.setActive();
    } else {
      api.getPanel('chat')?.api.close();
    }
  });
}
</script>

<template>
  <div class="dock">
    <div v-if="sessionError" class="session-error" role="alert">
      {{ sessionError }}
    </div>
    <DockviewVue
      class="dockview-theme-abyss"
      style="width: 100%; height: 100%"
      :components="components"
      @ready="onReady"
    />
  </div>
</template>

<style scoped>
.dock { height: 100%; min-height: 0; position: relative; }
.session-error {
  position: absolute;
  top: 8px;
  right: 8px;
  z-index: 20;
  max-width: 60%;
  padding: 6px 12px;
  background: #5b1e3fdd;
  color: #ffb4c8;
  font-size: 12px;
  border-radius: 6px;
}
</style>
