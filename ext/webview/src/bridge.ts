import type { ChatEvent } from '@vanyline/protocol/generated/chat-event';

/**
 * Pont webview↔host — côté webview (protocole figé en tâche 04a, relais RPC
 * whitelisté côté host dans `ext/src/panels/bridge.ts`).
 *
 * Fabrique à deps injectées (`BridgeDeps`) : `acquireVsCodeApi()` n'existe pas
 * sous vitest et n'est appelable qu'une seule fois par webview — le client est
 * donc mémoïsé à l'appel de `getBridgeClient()`, JAMAIS construit à l'import du
 * module (App.vue l'appelle dans son setup).
 */

declare global {
  interface Window {
    /** Injectée par VS Code dans la webview ; appelable une seule fois. */
    acquireVsCodeApi: () => { postMessage(message: unknown): void };
  }
}

/** Erreur renvoyée par le host dans une réponse ok:false (BridgeError du bridge host). */
export class BridgeRpcError extends Error {
  /** Identifiant propagé par le host : 'VNL-RPC-002', 'VNL-EXT-021'… null si inconnu. */
  readonly code: string | null;

  constructor(code: string | null, message: string) {
    super(message);
    this.name = 'BridgeRpcError';
    this.code = code;
    Object.setPrototypeOf(this, BridgeRpcError.prototype);
  }
}

export interface BridgeDeps {
  /** acquireVsCodeApi().postMessage (via le singleton en production). */
  post: (msg: unknown) => void;
  /** window 'message' → cb(event.data) ; retourne le désabonnement. */
  listen: (cb: (data: unknown) => void) => () => void;
}

/** Notification `chat/event` relayée par le host (une par conversation, seq croissant). */
export interface ChatEventParams {
  conversationId: string;
  seq: number;
  event: ChatEvent;
}

export interface BridgeClient {
  /** Relais {type:'rpc'} (méthodes whitelistées côté host). Résout result, rejette
   *  BridgeRpcError(code, message) sur ok:false. Un reqId monotone couple req/resp. */
  request<T = unknown>(method: string, params?: unknown): Promise<T>;
  /** {type:'chat/send'} nommé ; la réponse n'arrive qu'en FIN de tour. */
  chatSend(p: { conversationId: string; message: string; agent?: string }): Promise<unknown>;
  /** {type:'chat/cancel'} nommé ; la réponse arrive en rpc/resp. */
  chatCancel(conversationId: string): Promise<unknown>;
  /** Abonnement notifications chat/event (toutes conversations ; filtrer chez soi). */
  onChatEvent(cb: (p: ChatEventParams) => void): () => void;
  /** Abonnement messages typés host→webview ('session/new' | 'session/pick'). */
  onMessage(
    type: 'session/new' | 'session/pick',
    cb: (conversationId: string | null) => void,
  ): () => void;
  /** Abonnement notifications host `config/changed` (tous domaines ; le
   *  consommateur filtre s'il veut). Message valide : type exact + `domain`
   *  string — tout le reste silencieusement ignoré (convention F3). */
  onConfigChanged(cb: (domain: string) => void): () => void;
}

interface Pending {
  resolve: (value: unknown) => void;
  reject: (err: unknown) => void;
}

function isRecord(raw: unknown): raw is Record<string, unknown> {
  return typeof raw === 'object' && raw !== null && !Array.isArray(raw);
}

function toBridgeError(raw: unknown): BridgeRpcError {
  const err = isRecord(raw) ? raw : {};
  const code = typeof err.code === 'string' ? err.code : null;
  const message = typeof err.message === 'string' ? err.message : 'erreur du pont sans message';
  return new BridgeRpcError(code, message);
}

/** Fabrique testable. Corrélation reqId : Map interne, supprimée à la réponse. */
export function createBridgeClient(deps: BridgeDeps): BridgeClient {
  let nextReqId = 1;
  const pending = new Map<number, Pending>();
  const chatEventSubs = new Set<(p: ChatEventParams) => void>();
  const configChangedSubs = new Set<(domain: string) => void>();
  const typedSubs = new Map<'session/new' | 'session/pick', Set<(id: string | null) => void>>();

  const send = <T>(msg: Record<string, unknown>): Promise<T> => {
    const reqId = msg.reqId as number;
    return new Promise<T>((resolve, reject) => {
      // Enregistrement AVANT post : une réponse synchrone (tests) trouve le pending.
      pending.set(reqId, { resolve: resolve as (value: unknown) => void, reject });
      deps.post(msg);
    });
  };

  const handleMessage = (data: unknown): void => {
    if (!isRecord(data) || typeof data.type !== 'string') return;

    switch (data.type) {
      case 'rpc/resp':
      case 'chat/send/resp': {
        const reqId = data.reqId;
        if (typeof reqId !== 'number') return;
        const entry = pending.get(reqId);
        // reqId inconnu (réponse tardive après restart du host) → silencieusement ignorée.
        if (!entry) return;
        pending.delete(reqId);
        if (data.ok === true) {
          entry.resolve(data.result);
        } else {
          entry.reject(toBridgeError(data.error));
        }
        return;
      }
      case 'chat/event': {
        if (
          typeof data.conversationId !== 'string' ||
          typeof data.seq !== 'number' ||
          !isRecord(data.event)
        ) {
          return;
        }
        const params: ChatEventParams = {
          conversationId: data.conversationId,
          seq: data.seq,
          event: data.event as unknown as ChatEvent,
        };
        for (const cb of [...chatEventSubs]) cb(params);
        return;
      }
      case 'session/new':
      case 'session/pick': {
        const conversationId = data.conversationId;
        if (typeof conversationId !== 'string' && conversationId !== null) return;
        const subs = typedSubs.get(data.type);
        if (!subs) return;
        for (const cb of [...subs]) cb(conversationId);
        return;
      }
      case 'config/changed': {
        // Validation stricte (convention F3) : `domain` string exigé, sinon message avalé.
        if (typeof data.domain !== 'string') return;
        for (const cb of [...configChangedSubs]) cb(data.domain);
        return;
      }
      default:
        // Types inconnus (script tiers dans la webview, messages d'extension…) : ignorés.
        return;
    }
  };

  // Abonnement global unique, vie du client = vie de la webview : jamais désabonné
  // volontairement (le retour de listen est sciemment jeté).
  void deps.listen(handleMessage);

  const request = <T>(method: string, params?: unknown): Promise<T> => {
    const reqId = nextReqId++;
    const msg: Record<string, unknown> = { type: 'rpc', reqId, method };
    if (params !== undefined) {
      msg.params = params;
    }
    return send<T>(msg);
  };

  const chatSend = (p: {
    conversationId: string;
    message: string;
    agent?: string;
  }): Promise<unknown> => {
    const reqId = nextReqId++;
    const msg: Record<string, unknown> = {
      type: 'chat/send',
      reqId,
      conversationId: p.conversationId,
      message: p.message,
    };
    // JSON strict (comme 04a) : ne jamais poster `agent: undefined`.
    if (p.agent !== undefined) {
      msg.agent = p.agent;
    }
    return send(msg);
  };

  const chatCancel = (conversationId: string): Promise<unknown> => {
    const reqId = nextReqId++;
    return send({ type: 'chat/cancel', reqId, conversationId });
  };

  const onChatEvent = (cb: (p: ChatEventParams) => void): (() => void) => {
    chatEventSubs.add(cb);
    return () => {
      chatEventSubs.delete(cb);
    };
  };

  const onMessage = (
    type: 'session/new' | 'session/pick',
    cb: (conversationId: string | null) => void,
  ): (() => void) => {
    let set = typedSubs.get(type);
    if (!set) {
      set = new Set();
      typedSubs.set(type, set);
    }
    const subs = set;
    subs.add(cb);
    return () => {
      subs.delete(cb);
    };
  };

  const onConfigChanged = (cb: (domain: string) => void): (() => void) => {
    configChangedSubs.add(cb);
    return () => {
      configChangedSubs.delete(cb);
    };
  };

  return { request, chatSend, chatCancel, onChatEvent, onMessage, onConfigChanged };
}

let singleton: BridgeClient | undefined;

/** Singleton de production : premier appel → createBridgeClient sur acquireVsCodeApi
 *  + window 'message'. Jamais à l'import du module (cf. commentaire d'en-tête) ;
 *  App.vue l'appelle dans son setup. `resetBridgeSingleton()` pour les tests. */
export function getBridgeClient(): BridgeClient {
  if (!singleton) {
    const api = window.acquireVsCodeApi();
    singleton = createBridgeClient({
      post: (msg: unknown): void => {
        api.postMessage(msg);
      },
      listen: (cb: (data: unknown) => void): (() => void) => {
        const handler = (event: MessageEvent): void => {
          cb(event.data);
        };
        window.addEventListener('message', handler);
        return () => {
          window.removeEventListener('message', handler);
        };
      },
    });
  }
  return singleton;
}

export function resetBridgeSingleton(): void {
  singleton = undefined;
}
