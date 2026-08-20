<script setup lang="ts">
import { onMounted, onBeforeUnmount, useTemplateRef, inject, ref, watch } from 'vue';
import { EditorView, basicSetup } from 'codemirror';
import { keymap } from '@codemirror/view';
import { EditorState, StateEffect } from '@codemirror/state';
import type { LSPClient } from '@codemirror/lsp-client';
import { jumpToDefinition } from '@codemirror/lsp-client';
import { renameSymbolFromView } from '../../api/lspRename';
import { oneDark } from '@codemirror/theme-one-dark';
import { search, openSearchPanel } from '@codemirror/search';
import { languageExtensionForPath, lspToolchainForPath } from './editorLanguage';
import { registerIdeActions } from '../../composables/useIdeSession';
import type { DockviewPanelApi } from 'dockview-vue';
import type { Ref } from 'vue';
import type { SandboxFsClient } from '../../api/sandboxWs';
import ContextMenu, { type ContextMenuEntry } from '../ContextMenu.vue';

// Un panel Editor par fichier ouvert (IdeShell.openFile) : le chemin est
// fixe pour la durée de vie de cette instance — pas un ref partagé entre
// onglets (cf. docs/architecture.md, section Frontend, limite levée).
//
// dockview-vue (composants enregistrés via `components:`, pas via slot) ne
// lie qu'UN SEUL prop réel sur le composant de contenu : `params`, dont la
// valeur enveloppe TOUT — `{ params: <params passé à addPanel>, api,
// containerApi, tabLocation }` (vérifié à l'exécution, cf. commit qui a
// introduit ce commentaire : un `defineProps<{ params; api }>()` avec `api`
// en prop top-level séparée — pattern documenté pour `gridview`, pas pour
// `dockview` — laissait `props.api` systématiquement `undefined`).
const props = defineProps<{
  params: { params: { path: string }; api: DockviewPanelApi };
}>();
const filePath = props.params.params.path;
const panelApi = props.params.api;

// Fourni par IdeShell (task-05) : 'sandbox-fs' : Ref<SandboxFsClient | null>
const fsClient = inject<Ref<SandboxFsClient | null>>('sandbox-fs', ref(null) as Ref<SandboxFsClient | null>);

// Fourni par IdeShell : get-or-create d'un client LSP par toolchain pour cette sandbox.
// `path` sert à dériver le rootUri (cf. dirRootUri) — n'a d'effet qu'à la création du
// client pour ce toolchain (session partagée, cf. api/lsp.ts). Défaut
// `async () => null` (tests / IDE sans provider) → mode dégradé.
const getLspClient = inject<(toolchain: string, path: string) => Promise<LSPClient | null>>(
  'get-lsp-client',
  async () => null,
);

const denseTheme = EditorView.theme({
  '&': { height: '100%', fontSize: '12.5px' },
  '.cm-content': {
    fontFamily: 'ui-monospace, "SFMono-Regular", Menlo, Consolas, monospace',
    padding: '8px 0',
  },
  '.cm-gutters': {
    fontFamily: 'ui-monospace, "SFMono-Regular", Menlo, Consolas, monospace',
    border: 'none',
  },
  '.cm-scroller': { overflow: 'auto' },
});

const hostRef = useTemplateRef<HTMLDivElement>('host');
let view: EditorView | undefined;

/** Extensions communes à tout fichier — le langage (dépendant du chemin
 *  ouvert) est ajouté séparément par `loadFile`, cf. `editorLanguage.ts`. */
const baseExtensions = [basicSetup, oneDark, denseTheme, saveKeymap(), search({ top: true })];

// Visible le temps d'informer l'utilisateur — un échec de save/read silencieux
// laisserait croire que l'édition est enregistrée alors qu'elle ne l'est pas.
const statusMessage = ref<string | null>(null);
let statusTimer: ReturnType<typeof setTimeout> | undefined;
function showStatus(message: string) {
  statusMessage.value = message;
  clearTimeout(statusTimer);
  statusTimer = setTimeout(() => {
    statusMessage.value = null;
  }, 4000);
}

/** Ctrl+S / Cmd+S : écrit le document courant sur la sandbox (op write, contenu brut). */
function save() {
  const path = filePath;
  if (!view || !path || !fsClient.value) return;
  const content = view.state.doc.toString();
  fsClient.value
    .request<{ ok: boolean }>('write', { path, content })
    .catch((e: unknown) => {
      showStatus(`Échec de l'enregistrement : ${e instanceof Error ? e.message : String(e)}`);
    });
}

/** Le clic droit ne déplace pas nativement le curseur CodeMirror — sans ça, les
 *  actions du menu contextuel qui dépendent de la sélection (Aller à la définition,
 *  Renommer) opèrent sur la position du dernier clic *gauche*, pas sur l'endroit
 *  cliqué. Repositionne la sélection au point cliqué avant que le menu ne s'ouvre
 *  (le listener natif tourne avant celui de reka-ui, posé sur un ancêtre). */
function onEditorContextMenu(event: MouseEvent): void {
  if (!view) return;
  const pos = view.posAtCoords({ x: event.clientX, y: event.clientY });
  if (pos !== null) view.dispatch({ selection: { anchor: pos } });
}

/** Ctrl/Cmd+clic → aller à la définition. `@codemirror/lsp-client` ne fournit pas
 *  ce geste par défaut (juste F12/menu contextuel) — ajouté en plus, F12 reste
 *  valide. Repositionne la sélection au point cliqué (même besoin que le clic
 *  droit ci-dessus) avant d'appeler `jumpToDefinition`. */
function onEditorMousedown(event: MouseEvent): void {
  if (!view || !(event.ctrlKey || event.metaKey)) return;
  const pos = view.posAtCoords({ x: event.clientX, y: event.clientY });
  if (pos === null) return;
  event.preventDefault();
  view.dispatch({ selection: { anchor: pos } });
  jumpToDefinition(view);
}

const editorEntries: ContextMenuEntry[] = [
  { label: 'Couper', shortcut: '⌘X', action: cutSelection },
  { label: 'Copier', shortcut: '⌘C', action: copySelection },
  { label: 'Coller', shortcut: '⌘V', action: pasteClipboard },
  { sep: true },
  {
    label: 'Aller à la définition',
    shortcut: 'F12',
    action: () => {
      if (view) jumpToDefinition(view);
    },
  },
  {
    label: 'Renommer le symbole',
    shortcut: 'F2',
    action: () => {
      if (view && fsClient.value) {
        void renameSymbolFromView(view, fsClient.value).then((msg) => {
          if (msg) showStatus(msg);
        });
      }
    },
  },
  { sep: true },
  { label: 'Copier le chemin du fichier', action: copyFilePath },
];

function copySelection(): void {
  if (!view) return;
  const { from, to } = view.state.selection.main;
  if (from === to) return;
  if (!navigator.clipboard) { showStatus('Presse-papiers indisponible'); return; }
  void navigator.clipboard.writeText(view.state.sliceDoc(from, to))
    .catch((e: unknown) => showStatus(`Copie impossible : ${msg(e)}`));
}

function cutSelection(): void {
  if (!view) return;
  const { from, to } = view.state.selection.main;
  if (from === to) return;
  if (!navigator.clipboard) { showStatus('Presse-papiers indisponible'); return; }
  const text = view.state.sliceDoc(from, to);
  void navigator.clipboard.writeText(text)
    .then(() => { view?.dispatch({ changes: { from, to, insert: '' } }); })
    .catch((e: unknown) => showStatus(`Coupe impossible : ${msg(e)}`));
}

function pasteClipboard(): void {
  if (!view) return;
  if (!navigator.clipboard) { showStatus('Presse-papiers indisponible'); return; }
  const { from, to } = view.state.selection.main;
  void navigator.clipboard.readText()
    .then((text) => { view?.dispatch({ changes: { from, to, insert: text } }); })
    .catch((e: unknown) => showStatus(`Collage impossible : ${msg(e)}`));
}

function copyFilePath(): void {
  if (!navigator.clipboard) { showStatus('Presse-papiers indisponible'); return; }
  void navigator.clipboard.writeText(filePath)
    .catch((e: unknown) => showStatus(`Copie impossible : ${msg(e)}`));
}

function msg(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** Exposé pour les tests (fallback si keydown jsdom est fragile). */
defineExpose({ save, copySelection, cutSelection, pasteClipboard, copyFilePath, getView: () => view });

function saveKeymap() {
  return keymap.of([
    {
      key: 'Mod-s',
      run: () => {
        save();
        return true;
      },
      preventDefault: true,
    },
  ]);
}

/** Lit le fichier (mode raw — contenu réel, cf. task-01) et remplace le document. */
async function loadFile(path: string): Promise<void> {
  const fs = fsClient.value;
  if (!view || !fs) return;
  try {
    const resp = await fs.request<{ ok: boolean; content: string }>('read', {
      path,
      raw: true,
    });
    const langExts = languageExtensionForPath(path);
    // 1. Afficher le document immédiatement (pas d'attente du LSP).
    view.setState(
      EditorState.create({
        doc: resp.content,
        extensions: [...baseExtensions, ...langExts],
      }),
    );
    // 2. Puis, si une toolchain LSP couvre ce chemin, attendre le client et
    //    RECONFIGURER l'état avec le plugin (diagnostics/hover/complétion/keymaps).
    const lsp = lspToolchainForPath(path);
    if (lsp) {
      const client = await getLspClient(lsp.toolchain, path);
      if (client && view) {
        view.dispatch({ effects: StateEffect.reconfigure.of([
          ...baseExtensions,
          ...langExts,
          client.plugin(`file:///${path}`, lsp.languageId),
        ]) });
      }
    }
  } catch (e) {
    showStatus(
      `Impossible d'ouvrir le fichier : ${e instanceof Error ? e.message : String(e)}`,
    );
  }
}

// `saveActiveFile` doit cibler l'onglet visible, pas "le dernier Editor
// monté" — avec un panel par fichier, plusieurs instances coexistent.
// Chaque instance ne (ré)enregistre son `save` que lorsqu'elle devient
// active (cf. useIdeSession.registerIdeActions : fusion, dernier appelant
// gagne pour la même clé — l'instance active gagne donc bien la course).
function claimSaveActionIfActive() {
  if (panelApi.isActive) {
    registerIdeActions({
      saveActiveFile: save,
      findInActiveFile: () => { if (view) openSearchPanel(view); },
      replaceInActiveFile: () => { if (view) openSearchPanel(view); },
    });
  }
}
let activeChangeDisposable: { dispose(): void } | undefined;
let stopFsClientWatch: (() => void) | undefined;

onMounted(() => {
  view = new EditorView({
    state: EditorState.create({
      doc: '',
      extensions: baseExtensions,
    }),
    parent: hostRef.value!,
  });
  // `fsClient` (injecté depuis IdeShell) démarre à `null` et ne se résout
  // qu'après un aller-retour réseau (ticket + handshake WS) — restaurer un
  // layout dockview sauvegardé (`IdeShell.onReady` → `api.fromJSON`) recrée
  // les panels Editor **avant** que cette connexion soit établie. Sans ce
  // watch, `loadFile` s'exécutait une fois pour toutes au montage, voyait
  // `fs === null` et abandonnait en silence — le fichier ne se chargeait
  // jamais, sans erreur ni requête visible (bug trouvé en usage réel sur
  // cluster, latence réseau jamais reproduite en local/tests). `immediate`
  // couvre le cas déjà connecté (ouverture normale depuis l'Explorer, bien
  // après le montage de l'IDE) ; l'arrêt manuel dès le premier succès évite
  // un rechargement si `fsClient` change à nouveau plus tard.
  if (filePath) {
    stopFsClientWatch = watch(
      fsClient,
      (fs) => {
        if (!fs) return;
        stopFsClientWatch?.();
        void loadFile(filePath);
      },
      { immediate: true },
    );
  }
  claimSaveActionIfActive();
  activeChangeDisposable = panelApi.onDidActiveChange(claimSaveActionIfActive);
});

onBeforeUnmount(() => {
  activeChangeDisposable?.dispose();
  stopFsClientWatch?.();
  view?.destroy();
  view = undefined;
  clearTimeout(statusTimer);
});
</script>

<template>
  <div class="editor-wrap">
    <ContextMenu :entries="editorEntries" fill>
      <div
        ref="host"
        class="editor-host"
        @contextmenu="onEditorContextMenu"
        @mousedown="onEditorMousedown"
      ></div>
    </ContextMenu>
    <div v-if="statusMessage" class="editor-status" role="alert">{{ statusMessage }}</div>
  </div>
</template>

<!-- styles inchangés -->
<style scoped>
.editor-wrap {
  height: 100%;
  position: relative;
}
.editor-host {
  height: 100%;
  overflow: hidden;
}
:deep(.cm-editor) {
  height: 100%;
}
.editor-status {
  position: absolute;
  bottom: 8px;
  right: 8px;
  max-width: 70%;
  padding: 6px 12px;
  background: #5b1e3fdd;
  color: #ffb4c8;
  font-size: 12px;
  border-radius: 6px;
  pointer-events: none;
}
</style>
