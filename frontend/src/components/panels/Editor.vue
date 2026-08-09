<script setup lang="ts">
import { onMounted, onBeforeUnmount, useTemplateRef } from 'vue';
import { EditorView, basicSetup } from 'codemirror';
import { EditorState } from '@codemirror/state';
import { python } from '@codemirror/lang-python';
import { oneDark } from '@codemirror/theme-one-dark';

const code = `import shutil, hashlib
from pathlib import Path

# Recopie la bibliothèque depuis le NAS puis déclenche transcode.py
def sync_library(source: Path, dest: Path) -> int:
    changed = 0
    for f in source.rglob('*.mkv'):
        target = dest / f.relative_to(source)
        if not target.exists() or _digest(f) != _digest(target):
            shutil.copy2(f, target)
            changed += 1
    return changed
`;

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

onMounted(() => {
  view = new EditorView({
    state: EditorState.create({
      doc: code,
      extensions: [basicSetup, python(), oneDark, denseTheme],
    }),
    parent: hostRef.value!,
  });
});

onBeforeUnmount(() => view?.destroy());
</script>

<template>
  <div ref="host" class="editor-host"></div>
</template>

<style scoped>
.editor-host {
  height: 100%;
  overflow: hidden;
}
:deep(.cm-editor) {
  height: 100%;
}
</style>
