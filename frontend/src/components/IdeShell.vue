<script setup lang="ts">
import { onBeforeUnmount, onMounted, provide, ref, shallowRef, watch } from 'vue';
import { DockviewVue, type DockviewReadyEvent, type VueComponent, type ContextMenuItem, type GetTabContextMenuItemsParams } from 'dockview-vue';
import Explorer from './panels/Explorer.vue';
import Editor from './panels/Editor.vue';
import Workflow from './panels/Workflow.vue';
import Chat from './panels/Chat.vue';
import Terminal from './panels/Terminal.vue';
import GitPanel from './panels/GitPanel.vue';
import DiffView from './panels/DiffView.vue';
import { openSandboxWs, SandboxFsClient } from '../api/sandboxWs';
import { getLspClient, disposeLspClients } from '../api/lsp';
import { makeFileChangedHandler, makeFlushRequestHandler } from './panels/editorAutosave';
import { dirRootUri } from './panels/editorLanguage';
import { debounce, loadLayout, saveLayout } from './ideLayoutPersistence';
import { clearIdeActions, registerIdeActions, useIdeSession } from '../composables/useIdeSession';

const props = defineProps<{ sandboxName: string }>();

// Client /ws/fs partagé Explorer/Editor — UNE instance par sandbox ouverte.
// shallowRef : le client contient un WebSocket brut qu'il ne faut pas rendre
// profondément réactif. null tant que le ticket/minage n'est pas résolu.
const fsClient = shallowRef<SandboxFsClient | null>(null);
// api dockview — résolue par onReady, nécessaire à openFile() (appelée par
// Explorer via provide, potentiellement avant que ready n'ait eu lieu).
const dockviewApi = shallowRef<DockviewReadyEvent['api'] | null>(null);

provide('sandbox-fs', fsClient);
provide('sandbox-name', props.sandboxName);
provide('get-lsp-client', (toolchain: string, path: string) =>
  getLspClient(props.sandboxName, toolchain, dirRootUri(path), openFile));
// Handler fourni à Explorer : ouvre (ou active) l'onglet Editor du fichier —
// un panel dockview par fichier, cf. openFile ci-dessous.
provide('open-file', openFile);
// Handler fourni à Explorer : ferme l'onglet Editor d'un chemin s'il est ouvert.
provide('close-file', closeFile);
// Handler fourni au reste de l'IDE : ouvre (ou active) un onglet Diff.
provide('open-diff', openDiff);

const fsVersion = ref(0);
function notifyFsChange() { fsVersion.value++; }
provide('fs-version', fsVersion);
provide('notify-fs-change', notifyFsChange);

const { activeConversationId, sessionError } = useIdeSession();

onMounted(() => {
  openSandboxWs(props.sandboxName, '/ws/fs')
    .then((ws) => {
      const client = new SandboxFsClient(ws);
      // Canal push /ws/fs (tâche 08b) : un `file-changed` émis par le serveur
      // (edit_and_check en 08d) recharge le buffer ouvert, si l'onglet n'a
      // pas de frappe en attente de flush. Désabonnement implicite : la liste
      // d'abonnés vit dans le client, qui meurt avec le WS.
      client.onEvent('file-changed', makeFileChangedHandler(() => fsClient.value));
      // Aller-retour « flush avant écriture » (tâche 08c, cas B R1 sq3) : le
      // serveur (edit_and_check en 08d) réclame le flush d'un path et attend
      // l'ack avant d'écrire. L'ack part par la queue du client — jamais
      // devant le write du flush (FIFO), cf. makeFlushRequestHandler.
      client.onEvent('flush-request', makeFlushRequestHandler(() => fsClient.value));
      fsClient.value = client;
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
  disposeLspClients(props.sandboxName);
});

const components = {
  explorer: Explorer,
  editor: Editor,
  workflow: Workflow,
  chat: Chat,
  terminal: Terminal,
  git: GitPanel,
  diff: DiffView,
} as unknown as Record<string, VueComponent>;

/** Id de panel dockview pour un fichier ouvert — un panel Editor par fichier
 *  (task multi-onglets), stable pour retrouver un fichier déjà ouvert via
 *  api.getPanel(). */
function editorPanelId(path: string): string {
  return `editor:${path}`;
}

function basename(path: string): string {
  const idx = path.lastIndexOf('/');
  return idx === -1 ? path : path.slice(idx + 1);
}

/** Ancre pour poser un panel dans le groupe centre (rôle "éditeurs" : onglets
 *  fichiers + Workflow) : un onglet fichier déjà ouvert en priorité (le
 *  groupe existe déjà, le référencer le garde stable), sinon 'workflow'
 *  (présent par défaut, seul panel fixe du groupe), sinon aucune ancre —
 *  dockview place alors le panel lui-même (cas dégradé : groupe centre
 *  entièrement vidé par l'utilisateur). */
function centerAnchor(api: DockviewReadyEvent['api']): string | undefined {
  const openFilePanel = api.panels.find((p) => p.id.startsWith('editor:'));
  if (openFilePanel) return openFilePanel.id;
  if (api.getPanel('workflow')) return 'workflow';
  return undefined;
}

/** `position` prêt à l'emploi pour `addPanel`, relatif au groupe centre —
 *  `undefined` (pas de `position`) si aucune ancre n'existe, cas dégradé où
 *  dockview place le panel lui-même. */
function relativeToCenter(
  api: DockviewReadyEvent['api'],
  direction: 'left' | 'right' | 'below' | 'within',
): { referencePanel: string; direction: typeof direction } | undefined {
  const anchor = centerAnchor(api);
  return anchor ? { referencePanel: anchor, direction } : undefined;
}

/** Ouvre l'onglet Editor d'un fichier — un panel dockview par fichier
 *  (id `editor:<path>`) : réactive l'onglet s'il existe déjà, le crée sinon
 *  dans le groupe centre (cf. centerAnchor). Résolu au moment de l'appel
 *  (dockviewApi peut ne pas encore être prête si Explorer appelle avant
 *  onReady — no-op silencieux dans ce cas, l'Explorer n'est de toute façon
 *  affichable qu'une fois l'IDE monté). */
function openFile(path: string) {
  const api = dockviewApi.value;
  if (!api) return;
  const id = editorPanelId(path);
  const existing = api.getPanel(id);
  if (existing) {
    existing.api.setActive();
    return;
  }
  api.addPanel({
    id,
    component: 'editor',
    title: basename(path),
    params: { path },
    position: relativeToCenter(api, 'within'),
  });
  api.getPanel(id)?.api.setActive();
}

/** Ferme l'onglet Editor d'un chemin s'il est ouvert (renommage/suppression
 *  côté Explorer : le panel `editor:<path>` devient obsolète). */
function closeFile(path: string): void {
  const api = dockviewApi.value;
  if (!api) return;
  api.getPanel(editorPanelId(path))?.api.close();
}

/** Id de panel dockview pour un diff de fichier — `diff:<path>`. */
function diffPanelId(path: string): string {
  return `diff:${path}`;
}

/** Ouvre l'onglet Diff d'un fichier — même pattern qu'openFile : réactive
 *  l'onglet s'il existe, le crée sinon dans le groupe centre (même ancrage
 *  que editor:<path>). `staged` optionnel transmis à DiffView via params. */
function openDiff(path: string, staged?: boolean) {
  const api = dockviewApi.value;
  if (!api) return;
  const id = diffPanelId(path);
  const existing = api.getPanel(id);
  if (existing) {
    existing.api.setActive();
    return;
  }
  api.addPanel({
    id,
    component: 'diff',
    title: `Diff · ${basename(path)}`,
    params: { path, staged },
    position: relativeToCenter(api, 'within'),
  });
  api.getPanel(id)?.api.setActive();
}

/** Copie le chemin relatif d'un fichier ouvert dans un onglet éditeur. */
function copyPanelPath(panelId: string): void {
  if (!navigator.clipboard) return;
  const path = panelId.startsWith('editor:') ? panelId.slice('editor:'.length) : '';
  if (!path) return;
  void navigator.clipboard.writeText(path).catch(() => {});
}

/** Menu contextuel natif des onglets : Fermer / Fermer les autres / Fermer
 *  tout, séparateur, puis « Copier le chemin » pour les onglets éditeur
 *  (`editor:<path>`). */
function getTabContextMenuItems(params: GetTabContextMenuItemsParams): ContextMenuItem[] {
  const items: ContextMenuItem[] = ['close', 'closeOthers', 'closeAll', 'separator'];
  if (params.panel.id.startsWith('editor:')) {
    items.push({ label: 'Copier le chemin', action: () => copyPanelPath(params.panel.id) });
  }
  return items;
}

function addDefaultPanels(api: DockviewReadyEvent['api']) {
  api.addPanel({
    id: 'explorer',
    component: 'explorer',
    title: 'Explorer',
    initialWidth: 230,
  });

  api.addPanel({
    id: 'git',
    component: 'git',
    title: 'Git',
    position: { referencePanel: 'explorer', direction: 'below' },
    initialHeight: 240,
  });

  api.addPanel({
    id: 'workflow',
    component: 'workflow',
    title: 'sync-media.dag',
    position: { referencePanel: 'explorer', direction: 'right' },
  });

  api.addPanel({
    id: 'terminal',
    component: 'terminal',
    title: 'Terminal',
    position: { referencePanel: 'workflow', direction: 'below' },
    initialHeight: 170,
  });

  // Pas de panel 'chat' par défaut : la colonne assistant n'existe que si
  // une vraie session agent est démarrée (cf. useIdeSession, watcher
  // ci-dessous). addChatPanel() la pose le moment venu.
  api.getPanel('workflow')?.api.setActive();
}

function addChatPanel(api: DockviewReadyEvent['api']) {
  if (api.getPanel('chat')) return;
  api.addPanel({
    id: 'chat',
    component: 'chat',
    title: 'Assistant',
    position: relativeToCenter(api, 'right'),
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
    position: relativeToCenter(api, 'left'),
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
    position: relativeToCenter(api, 'below'),
    initialHeight: 170,
  });
  api.getPanel(id)?.api.setActive();
}

function onReady(event: DockviewReadyEvent) {
  const { api } = event;
  dockviewApi.value = api;

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

defineExpose({ closeFile, openDiff, getTabContextMenuItems });
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
      :get-tab-context-menu-items="getTabContextMenuItems"
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
