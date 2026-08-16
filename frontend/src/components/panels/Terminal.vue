<script setup lang="ts">
import { onMounted, onBeforeUnmount, useTemplateRef, inject } from 'vue';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import '@xterm/xterm/css/xterm.css';
import ContextMenu, { type ContextMenuEntry } from '../ContextMenu.vue';
import { openSandboxWs } from '../../api/sandboxWs';
import { copySelection, pasteClipboard, setTerm, setWs } from './TerminalActions';

const sandboxName = inject<string>('sandbox-name', '');

const hostRef = useTemplateRef<HTMLDivElement>('host');
let term: Terminal | undefined;
let fit: FitAddon | undefined;
let resizeObserver: ResizeObserver | undefined;
let ws: WebSocket | undefined;

const terminalEntries: ContextMenuEntry[] = [
  { label: 'Copier', action: copySelection },
  { label: 'Coller', action: pasteClipboard },
];

defineExpose({ copySelection, pasteClipboard });

function sendResize() {
  if (!term || !ws || ws.readyState !== WebSocket.OPEN) return;
  ws.send(JSON.stringify({ type: 'resize', cols: term.cols, rows: term.rows }));
}

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
    scrollback: 200,
    // disableStdin retiré — l'entrée utilisateur part vers le PTY.
  });

  setTerm(term);

  fit = new FitAddon();
  term.loadAddon(fit);
  term.open(hostRef.value!);
  fit.fit();

  resizeObserver = new ResizeObserver(() => {
    fit?.fit();
    sendResize();
  });
  resizeObserver.observe(hostRef.value!);

  // Connexion binaire dédiée — un ticket propre pour /ws/terminal (jamais
  // réutilisé depuis /ws/fs, cf. sandbox-ws-runtime).
  openSandboxWs(sandboxName, '/ws/terminal')
    .then((socket) => {
      ws = socket;
      setWs(ws);
      // binaryType 'arraybuffer' : event.data des frames binaires est un
      // ArrayBuffer (sinon Blob) — nécessaire pour new Uint8Array(ev.data).
      ws.binaryType = 'arraybuffer';
      term!.onData((data) => ws!.send(new TextEncoder().encode(data)));
      ws.addEventListener('message', (ev: MessageEvent) => {
        if (typeof ev.data === 'string') return; // le serveur n'envoie que du binaire
        term!.write(new Uint8Array(ev.data));
      });
      // openSandboxWs ne résout qu'à l'event 'open' réel (cf. sandboxWs.ts) :
      // le socket est déjà OPEN ici, la taille initiale part immédiatement.
      sendResize();
    })
    .catch(() => {
      // ticket/ingress indisponible (dépendance d'infra) : terminal vide, pas de PTY.
    });
});

onBeforeUnmount(() => {
  resizeObserver?.disconnect();
  term?.dispose();
  ws?.close();
});
</script>

<template>
  <ContextMenu :entries="terminalEntries" :as-child="true">
    <div ref="host" class="terminal-host"></div>
  </ContextMenu>
</template>

<!-- styles inchangés (le mock n'écrit plus de lignes statiques) -->
<style scoped>
.terminal-host {
  height: 100%;
  padding: 6px 0 0 8px;
  background: #000c18;
}
</style>