<script setup lang="ts">
import { computed, inject, onBeforeUnmount, onMounted, useTemplateRef, ref, watch } from 'vue';
import { EditorView, basicSetup } from 'codemirror';
import { EditorState } from '@codemirror/state';
import { oneDark } from '@codemirror/theme-one-dark';
import { search } from '@codemirror/search';
import { EditorView as EditorViewMod } from '@codemirror/view';
import { EditorState as EditorStateMod } from '@codemirror/state';
import { unifiedMergeView } from '@codemirror/merge';
import { gitClient } from '../../api/gitClient';
import { languageExtensionForPath } from './editorLanguage';
import { reconstructBase } from './diffPatch';
import type { DockviewPanelApi } from 'dockview-vue';
import type { Ref } from 'vue';
import type { SandboxFsClient } from '../../api/sandboxWs';

const props = defineProps<{
  params: { params: { path: string; staged?: boolean }; api: DockviewPanelApi };
}>();
const filePath = props.params.params.path;
const staged = props.params.params.staged ?? false;

const fsClient = inject<Ref<SandboxFsClient | null>>('sandbox-fs', ref(null) as Ref<SandboxFsClient | null>);
const sandboxName = inject<string>('sandbox-name', '');

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

const baseExtensions = [basicSetup, oneDark, denseTheme, search({ top: true })];

const statusMessage = ref<string | null>(null);
let statusTimer: ReturnType<typeof setTimeout> | undefined;
function showStatus(message: string) {
  statusMessage.value = message;
  clearTimeout(statusTimer);
  statusTimer = setTimeout(() => {
    statusMessage.value = null;
  }, 4000);
}

function msg(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** Extensions de base + langage (dépendant du fichier). */
const langExts = computed(() => languageExtensionForPath(filePath));

/** Charge le diff (working tree + git diff) et configure le view avec
 *  unifiedMergeView. */
async function loadDiff(path: string): Promise<void> {
  const fs = fsClient.value;
  const name = sandboxName;
  if (!view || !fs || !name) return;
  try {
    const [rawRes, diffRes] = await Promise.all([
      fs.request<{ ok: boolean; content: string }>('read', { path, raw: true }),
      gitClient.diff(name, path, staged),
    ]);
    // reconstructBase est déjà importé en haut
    const base = reconstructBase(rawRes.content, diffRes.diff);

    // Si le patch a au moins un hunk @@ : rendu merge unifié
    if (diffRes.diff.includes('@@')) {
      view.setState(
        EditorState.create({
          doc: rawRes.content,
          extensions: [...baseExtensions, ...langExts.value,
            EditorViewMod.editable.of(false),
            EditorStateMod.readOnly.of(true),
            unifiedMergeView({ original: base }),
          ],
        }),
      );
    } else {
      // Pas de hunk → rendu texte simple en lecture seule
      view.setState(
        EditorState.create({
          doc: rawRes.content,
          extensions: [...baseExtensions, ...langExts.value,
            EditorViewMod.editable.of(false),
            EditorStateMod.readOnly.of(true),
          ],
        }),
      );
    }
  } catch (e) {
    showStatus(`Impossible d'ouvrir le diff : ${msg(e)}`);
  }
}

let stopFsClientWatch: (() => void) | undefined;

onMounted(() => {
  view = new EditorView({
    state: EditorState.create({
      doc: '',
      extensions: baseExtensions,
    }),
    parent: hostRef.value!,
  });
  if (filePath) {
    stopFsClientWatch = watch(
      fsClient,
      (fs) => {
        if (!fs) return;
        stopFsClientWatch?.();
        void loadDiff(filePath);
      },
      { immediate: true },
    );
  }
});

onBeforeUnmount(() => {
  stopFsClientWatch?.();
  view?.destroy();
  view = undefined;
  clearTimeout(statusTimer);
});

defineExpose({ getView: () => view, loadDiff });
</script>

<template>
  <div class="diff-wrap">
    <div ref="host" class="diff-host"></div>
    <div v-if="statusMessage" class="diff-status" role="alert">{{ statusMessage }}</div>
  </div>
</template>

<style scoped>
.diff-wrap {
  height: 100%;
  position: relative;
}
.diff-host {
  height: 100%;
  overflow: hidden;
}
:deep(.cm-editor) {
  height: 100%;
}
.diff-status {
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