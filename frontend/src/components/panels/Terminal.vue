<script setup lang="ts">
import { onMounted, onBeforeUnmount, useTemplateRef, inject } from 'vue';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import '@xterm/xterm/css/xterm.css';
import { openSandboxWs } from '../../api/sandboxWs';

const sandboxName = inject<string>('sandbox-name', '');

const hostRef = useTemplateRef<HTMLDivElement>('host');
let term: Terminal | undefined;
let fit: FitAddon | undefined;
let resizeObserver: ResizeObserver | undefined;
let ws: WebSocket | undefined;

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
      // binaryType 'arraybuffer' : event.data des frames binaires est un
      // ArrayBuffer (sinon Blob) — nécessaire pour new Uint8Array(ev.data).
      ws.binaryType = 'arraybuffer';
      term!.onData((data) => ws!.send(new TextEncoder().encode(data)));
      ws.addEventListener('message', (ev: MessageEvent) => {
        if (typeof ev.data === 'string') return; // le serveur n'envoie que du binaire
        term!.write(new Uint8Array(ev.data));
      });
      sendResize(); // synchronise la taille initiale après l'ouverture
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
  <div ref="host" class="terminal-host"></div>
</template>

<!-- styles inchangés (le mock n'écrit plus de lignes statiques) -->
<style scoped>
.terminal-host {
  height: 100%;
  padding: 6px 0 0 8px;
  background: #000c18;
}
</style>
