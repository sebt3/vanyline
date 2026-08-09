<script setup lang="ts">
import { onMounted, onBeforeUnmount, useTemplateRef } from 'vue';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import '@xterm/xterm/css/xterm.css';

const lines = [
  '\x1b[32m❯\x1b[0m python jobs/sync_library.py --source /mnt/nas/media --dest ./library',
  '[sync_library] scanning /mnt/nas/media…',
  '[sync_library] 214 fichiers, 6 modifiés depuis le dernier run',
  '[sync_library] copie: Documentaires/2025/passage-nord.mkv',
  '[sync_library] terminé — 6 fichiers copiés en 3.2s',
];

const hostRef = useTemplateRef<HTMLDivElement>('host');
let term: Terminal | undefined;
let fit: FitAddon | undefined;
let resizeObserver: ResizeObserver | undefined;

onMounted(() => {
  term = new Terminal({
    fontFamily: 'ui-monospace, "SFMono-Regular", Menlo, Consolas, monospace',
    fontSize: 12,
    theme: {
      background: '#000c18',
      foreground: '#9497a9',
      cursor: '#9497a9',
      selectionBackground: '#2b2b4a',
    },
    cursorBlink: true,
    disableStdin: true,
    scrollback: 200,
  });

  fit = new FitAddon();
  term.loadAddon(fit);
  term.open(hostRef.value!);
  fit.fit();

  lines.forEach((l) => term!.writeln(l));
  term!.write('\x1b[32m❯\x1b[0m ');

  resizeObserver = new ResizeObserver(() => fit?.fit());
  resizeObserver.observe(hostRef.value!);
});

onBeforeUnmount(() => {
  resizeObserver?.disconnect();
  term?.dispose();
});
</script>

<template>
  <div ref="host" class="terminal-host"></div>
</template>

<style scoped>
.terminal-host {
  height: 100%;
  padding: 6px 0 0 8px;
  background: #000c18;
}
</style>
