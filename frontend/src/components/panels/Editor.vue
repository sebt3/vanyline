<script setup lang="ts">
import { onMounted, onBeforeUnmount, useTemplateRef, inject, ref, watch } from 'vue';
import { EditorView, basicSetup } from 'codemirror';
import { keymap } from '@codemirror/view';
import { EditorState } from '@codemirror/state';
import { oneDark } from '@codemirror/theme-one-dark';
import { languageExtensionForPath } from './editorLanguage';
import { registerIdeActions } from '../../composables/useIdeSession';
import type { Ref } from 'vue';
import type { SandboxFsClient } from '../../api/sandboxWs';

// Fournis par IdeShell (task-05) :
// - 'sandbox-fs' : Ref<SandboxFsClient | null>
// - 'open-file-path' : Ref<string | null>
const fsClient = inject<Ref<SandboxFsClient | null>>('sandbox-fs', ref(null) as Ref<SandboxFsClient | null>);
const openFilePath = inject<Ref<string | null>>('open-file-path', ref(null) as Ref<string | null>);

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
const baseExtensions = [basicSetup, oneDark, denseTheme, saveKeymap()];

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
  const path = openFilePath?.value ?? null;
  if (!view || !path || !fsClient.value) return;
  const content = view.state.doc.toString();
  fsClient.value
    .request<{ ok: boolean }>('write', { path, content })
    .catch((e: unknown) => {
      showStatus(`Échec de l'enregistrement : ${e instanceof Error ? e.message : String(e)}`);
    });
}

/** Exposé pour les tests (fallback si keydown jsdom est fragile). */
defineExpose({ save });

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
    view.setState(
      EditorState.create({
        doc: resp.content,
        extensions: [...baseExtensions, ...languageExtensionForPath(path)],
      }),
    );
  } catch (e) {
    showStatus(
      `Impossible d'ouvrir le fichier : ${e instanceof Error ? e.message : String(e)}`,
    );
  }
}

watch(openFilePath, (path) => {
  if (path) void loadFile(path);
});

onMounted(() => {
  view = new EditorView({
    state: EditorState.create({
      doc: '',
      extensions: baseExtensions,
    }),
    parent: hostRef.value!,
  });
  if (openFilePath.value) void loadFile(openFilePath.value);
  // Fusionné avec les autres handlers (ex. closeActiveTab posé par
  // IdeShell) — cf. useIdeSession.ts. Pont pour le menu 'Enregistrer'.
  registerIdeActions({ saveActiveFile: save });
});

onBeforeUnmount(() => {
  view?.destroy();
  clearTimeout(statusTimer);
});
</script>

<template>
  <div class="editor-wrap">
    <div ref="host" class="editor-host"></div>
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
