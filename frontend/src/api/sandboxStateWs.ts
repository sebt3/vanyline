/** Sandbox state WS : URL same-origin (auth par cookie OIDC, jamais de ticket). */
function sandboxStateWsUrl(): string {
  const protocol = globalThis.location.protocol === 'https:' ? 'wss' : 'ws';
  return `${protocol}://${globalThis.location.host}/api/ws/sandbox-state`;
}

/** Event pushé par le backend (miroir du Rust, app/src/ws/sandbox_state.sdd) quand
 *  la phase d'une Sandbox ou d'un Project change. `kind` choisit la famille,
 *  `name` la ressource, `phase` vaut `null` en suppression. */
export interface SandboxStateEvent {
  kind: 'sandbox' | 'project';
  name: string;
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
