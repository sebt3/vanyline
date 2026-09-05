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
  path: '/ws/fs' | '/ws/terminal' | `/ws/lsp/${string}`,
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

/** Frame reçue sur /ws/fs : réponse de requête (id repris de la requête dès
 *  le serveur ≥ 08b), ou frame de push (`event` sans `id`). Les autres champs
 *  dépendent de l'op/l'événement. */
interface FsFrame {
  [key: string]: unknown;
  id?: string | number;
  event?: string;
}

/** Requête en vol — la queue sérialisée en garantit au plus UNE. La
 *  résolution livre la frame entière (le générique `T` de `request` est une
 *  promesse de l'appelant ; le runtime ne valide pas la forme). */
interface PendingRequest {
  id: number;
  resolve: (value: unknown) => void;
  reject: (reason: Error) => void;
}

/** Wrapper autour d'une connexion /ws/fs : sérialise les requêtes (file
 *  d'attente — une requête en vol à la fois, sémantique de la boucle
 *  serveur, task-08b) et route les frames reçues par un listener PERMANENT
 *  posé à la construction : réponse résolue par `id` de corrélation, push
 *  `event` notifié aux abonnés `onEvent`, et fallback legacy pour les
 *  serveurs sans corrélation. Partagé entre Explorer et Editor pour une même
 *  sandbox ouverte. */
export class SandboxFsClient {
  private queue: Promise<unknown> = Promise.resolve();
  private ws: WebSocket;
  private nextId = 1;
  private pending: PendingRequest | null = null;
  private handlers: Record<string, Array<(data: FsFrame) => void>> = {};

  constructor(ws: WebSocket) {
    this.ws = ws;
    // Listener permanent (remplace le addEventListener one-shot par requête,
    // qui rendait toute frame non sollicitée empoisonnable : elle résolvait
    // la requête en vol avec le mauvais payload).
    this.ws.addEventListener('message', this.onMessage);
  }

  /** Souscription aux frames de push (`{"event":…}`) : `handler` reçoit la
   *  frame entière (payload propre au type d'événement — d'où `any` au
   *  prototype, le runtime ne valide pas) ; plusieurs abonnés par type OK ;
   *  le retour désabonne. Un handler qui lève est contenu (try/catch) et
   *  n'empêche pas les autres abonnés — erreur avalée délibérément, sans
   *  console. */
  onEvent(type: string, handler: (data: any) => void): () => void {
    const list = (this.handlers[type] ??= []);
    list.push(handler);
    return () => {
      const arr = this.handlers[type];
      if (!arr) return;
      const idx = arr.indexOf(handler);
      if (idx !== -1) arr.splice(idx, 1);
    };
  }

  request<T>(op: string, params: Record<string, unknown>): Promise<T> {
    const id = this.nextId++;
    const run = () =>
      new Promise<T>((resolve, reject) => {
        this.pending = {
          id,
          resolve: resolve as (value: unknown) => void,
          reject,
        };
        this.ws.send(JSON.stringify({ op, id, ...params }));
      });
    this.queue = this.queue.then(run, run);
    return this.queue as Promise<T>;
  }

  /** Dispatch de toute frame texte : parse tolérant (non-JSON → ignoré),
   *  routage id / event / legacy — voir les branches inline. */
  private onMessage = (ev: MessageEvent): void => {
    let frame: FsFrame;
    try {
      frame = JSON.parse(String(ev.data));
    } catch {
      return; // non-JSON (binaire, protocole tiers) : ignoré
    }
    if (typeof frame !== 'object' || frame === null) return;

    if (frame.id !== undefined) {
      // Réponse corrélée : seule la requête en vol portant le MÊME id est
      // résolue ; un id inconnu (requête déjà soldée, ou frame égarée) est
      // ignoré — la requête en vol attend la sienne.
      if (this.pending && this.pending.id === frame.id) this.settle(frame);
      return;
    }
    if (typeof frame.event === 'string') {
      const list = this.handlers[frame.event];
      if (!list) return; // événement sans abonné : ignoré silencieusement
      // Copie itérative : un handler qui se désabonne ne doit pas sauter un
      // confrère dans la même notification.
      for (const handler of [...list]) {
        try {
          handler(frame);
        } catch {
          // Abonné malade : on continue les autres (documenté sur onEvent).
        }
      }
      return;
    }
    // Fallback legacy — RETIREABLE quand le serveur ≥ 08b est déployé
    // partout : un serveur sans champ de corrélation rend une réponse sans
    // id ni event ; la queue (une requête en vol à la fois) suffit à la
    // router. À supprimer alors, la branche `id` couvrant tout.
    if (this.pending) this.settle(frame);
  };

  /** Solde la requête en vol sur une frame de réponse (résolution/rejet
   *  exactement comme le listener one-shot historique). */
  private settle(frame: FsFrame): void {
    const pending = this.pending;
    if (!pending) return;
    this.pending = null;
    if (frame.ok) {
      pending.resolve(frame);
    } else {
      pending.reject(new Error(String(frame.error)));
    }
  }
}
