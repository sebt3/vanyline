<script setup lang="ts">
import { onMounted, provide, shallowRef, ref } from 'vue';
import { DockviewVue, type DockviewReadyEvent, type VueComponent } from 'dockview-vue';
import Explorer from './panels/Explorer.vue';
import Editor from './panels/Editor.vue';
import Workflow from './panels/Workflow.vue';
import Chat from './panels/Chat.vue';
import Terminal from './panels/Terminal.vue';
import { openSandboxWs, SandboxFsClient } from '../api/sandboxWs';
import { debounce, loadLayout, saveLayout } from './ideLayoutPersistence';

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

  api.addPanel({
    id: 'chat',
    component: 'chat',
    title: 'Assistant',
    position: { referencePanel: 'editor', direction: 'right' },
    initialWidth: 330,
  });

  api.getPanel('editor')?.api.setActive();
}

function onReady(event: DockviewReadyEvent) {
  const { api } = event;

  // Répartition sauvegardée pour cette sandbox : la restaurer plutôt que
  // rebâtir le layout par défaut. Un layout absent/corrompu retombe sur le
  // layout par défaut sans planter (cf. loadLayout).
  const saved = loadLayout(props.sandboxName);
  if (saved) {
    api.fromJSON(saved);
  } else {
    addDefaultPanels(api);
  }

  // onDidLayoutChange se déclenche à chaque frame d'un drag — anti-rebond
  // avant d'écrire dans localStorage.
  const persist = debounce(() => saveLayout(props.sandboxName, api.toJSON()), 400);
  api.onDidLayoutChange(persist);
}
</script>

<template>
  <div class="dock">
    <DockviewVue
      class="dockview-theme-abyss"
      style="width: 100%; height: 100%"
      :components="components"
      @ready="onReady"
    />
  </div>
</template>

<style scoped>
.dock { height: 100%; min-height: 0; }
</style>
