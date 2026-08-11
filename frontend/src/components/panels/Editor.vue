<script setup lang="ts">
import { onMounted, onBeforeUnmount, useTemplateRef, inject, ref, watch } from 'vue';
import { EditorView, basicSetup } from 'codemirror';
import { keymap } from '@codemirror/view';
import { EditorState } from '@codemirror/state';
import { python } from '@codemirror/lang-python';
import { oneDark } from '@codemirror/theme-one-dark';
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

const extensions = [basicSetup, python(), oneDark, denseTheme, saveKeymap()];

/** Ctrl+S / Cmd+S : écrit le document courant sur la sandbox (op write, contenu brut). */
function save() {
  const path = openFilePath?.value ?? null;
  if (!view || !path || !fsClient.value) return;
  const content = view.state.doc.toString();
  void fsClient.value.request<{ ok: boolean }>('write', { path, content });
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
        extensions,
      }),
    );
  } catch {
    // lecture impossible (fichier supprimé, confinement…) : on laisse le contenu actuel.
  }
}

watch(openFilePath, (path) => {
  if (path) void loadFile(path);
});

onMounted(() => {
  view = new EditorView({
    state: EditorState.create({
      doc: '',
      extensions,
    }),
    parent: hostRef.value!,
  });
  if (openFilePath.value) void loadFile(openFilePath.value);
});

onBeforeUnmount(() => view?.destroy());
</script>

<template>
  <div ref="host" class="editor-host"></div>
</template>

<!-- styles inchangés -->
<style scoped>
.editor-host {
  height: 100%;
  overflow: hidden;
}
:deep(.cm-editor) {
  height: 100%;
}
</style>
