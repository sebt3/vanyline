/** Sandbox state WS : URL same-origin (auth par cookie OIDC, jamais de ticket). */
function sandboxStateWsUrl(): string {
  const protocol = globalThis.location.protocol === 'https:' ? 'wss' : 'ws';
  return `${protocol}://${globalThis.location.host}/api/ws/sandbox-state`;
}

/** Event pushé par le backend quand le status.phase d'une sandbox change.
 *  `phase` vaut `null` en suppression (ou avant la première phase connue). */
export interface SandboxStateEvent {
  sandbox: string;
  phase: string | null;
}

/** Ouvre la connexion WS sandbox-state. Same-origin, auth par cookie OIDC. */
export function openSandboxStateWs(): Promise<WebSocket> {
  const ws = new WebSocket(sandboxStateWsUrl());
  return new Promise<WebSocket>((resolve, reject) => {
    const onOpen = () => {
      ws.removeEventListener('error', onError);
      resolve(ws);
    };
    const onError = (ev: Event) => {
      ws.removeEventListener('open', onOpen);
      reject(new Error(`sandbox-state WebSocket: ${ev.type}`));
    };
    ws.addEventListener('open', onOpen, { once: true });
    ws.addEventListener('error', onError, { once: true });
  });
}
