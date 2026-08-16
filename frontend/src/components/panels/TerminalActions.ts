import type { Terminal } from '@xterm/xterm';

let term: Terminal | undefined;
export function setTerm(t: Terminal | undefined) {
  term = t;
}
let ws: WebSocket | undefined;
export function setWs(w: WebSocket | undefined) {
  ws = w;
}

export function copySelection(): void {
  if (!term || !navigator.clipboard) return;
  const text = term.getSelection();
  if (!text) return;
  void navigator.clipboard.writeText(text).catch(() => {});
}

export function pasteClipboard(): void {
  if (!term || !ws || ws.readyState !== WebSocket.OPEN || !navigator.clipboard) return;
  void navigator.clipboard.readText()
    .then((text) => { ws?.send(new TextEncoder().encode(text)); })
    .catch(() => {});
}