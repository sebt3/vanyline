import { createApiClient } from './client';

const client = createApiClient();

/** Réponse de `POST /api/sandboxes/{name}/ws-ticket` : ticket court-vécu à
 *  usage unique + host public WS (`{name}.sandboxes.{application.host}`). */
interface WsTicketOut {
  ticket: string;
  wsHost: string;
}

/** Mine un ticket court-vécu à usage unique pour `sandboxName` et ouvre la
 *  connexion WS correspondante. Un ticket par connexion — jamais réutilisé
 *  (cf. sandbox-ws-runtime : consommé au premier GET /ws/* qui le présente).
 *
 *  La promesse ne résout qu'à l'event `open` réel — pas à la construction du
 *  `WebSocket` (état `CONNECTING`). Envoyer sur un socket `CONNECTING` lève
 *  une `InvalidStateError` : sans cette attente, tout appelant qui envoie
 *  dès la résolution (ex. `Explorer.vue` sur l'auto-expand de la racine)
 *  perd sa première requête silencieusement. */
export async function openSandboxWs(
  sandboxName: string,
  path: '/ws/fs' | '/ws/terminal',
): Promise<WebSocket> {
  const { ticket, wsHost } = await client.post<WsTicketOut>(
    `/api/sandboxes/${sandboxName}/ws-ticket`,
  );
  const ws = new WebSocket(`wss://${wsHost}${path}?ticket=${encodeURIComponent(ticket)}`);
  return new Promise<WebSocket>((resolve, reject) => {
    const onOpen = () => {
      ws.removeEventListener('error', onError);
      resolve(ws);
    };
    const onError = (ev: Event) => {
      ws.removeEventListener('open', onOpen);
      reject(new Error(`sandbox WebSocket ${path}: ${ev.type}`));
    };
    ws.addEventListener('open', onOpen, { once: true });
    ws.addEventListener('error', onError, { once: true });
  });
}

/** Wrapper autour d'une connexion /ws/fs : sérialise les requêtes (pas de
 *  champ de corrélation dans le protocole serveur — une requête en vol à la
 *  fois, contrainte du protocole, pas un choix arbitraire). Partagé entre
 *  Explorer et Editor pour une même sandbox ouverte. */
export class SandboxFsClient {
  private queue: Promise<unknown> = Promise.resolve();
  private ws: WebSocket;
  constructor(ws: WebSocket) { this.ws = ws; }

  request<T>(op: string, params: Record<string, unknown>): Promise<T> {
    const run = () =>
      new Promise<T>((resolve, reject) => {
        const handler = (ev: MessageEvent) => {
          this.ws.removeEventListener('message', handler);
          const data = JSON.parse(ev.data);
          data.ok ? resolve(data) : reject(new Error(data.error));
        };
        this.ws.addEventListener('message', handler);
        this.ws.send(JSON.stringify({ op, ...params }));
      });
    this.queue = this.queue.then(run, run);
    return this.queue as Promise<T>;
  }
}
