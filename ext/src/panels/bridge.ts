import { RpcError, type ChatEvent } from '@vanyline/protocol';

/**
 * Pont webview↔host — logique pure (zéro import `vscode`, zéro I/O).
 * Le provider (`chat.ts`) branche ces fonctions sur la vraie webview et le handle RPC ;
 * ici tout passe par `BridgeApi` (injecté), donc c'est intégralement testable sous vitest.
 *
 * Erreurs du pont (hors design doc — code -0XX, identifiants requis AGENTS.md) :
 *  VNL-EXT-020 : relais refusé (méthode hors whitelist, type de message inconnu) ;
 *  VNL-EXT-021 : serveur vanyline non démarré (supervisor.current() === undefined).
 */

/** Message d'erreur minimal relayable à la webview (pas d'objet RpcError brut). */
export interface BridgeError {
  readonly code: string | null; // data.code du RpcError (ex. 'VNL-RPC-002'), null si inconnu
  readonly message: string;
}

/** Notification relayée de l'hôte vers la webview (notification `chat/event` du RPC). */
export interface ChatEventMessage {
  type: 'chat/event';
  conversationId: string;
  seq: number;
  event: ChatEvent;
}

/** Relais RPC autorisés depuis la webview (sécurité : limiter la surface en cas de
 *  script injecté dans la webview). 'chat/send' ET 'chat/cancel' sont des messages nommés
 *  — les passer via {type:'rpc'} est REFUSÉ (-020). */
export const RELAY_WHITELIST: readonly string[] = [
  'conversations/list',
  'conversations/get',
  'conversations/create',
  'conversations/delete',
  'config/agents',
];

export type BridgeRequest =
  | { kind: 'chat/send'; reqId: number; conversationId: string; message: string; agent?: string }
  | { kind: 'chat/cancel'; reqId: number; conversationId: string }
  | { kind: 'rpc'; reqId: number; method: string; params: unknown };

function isRecord(raw: unknown): raw is Record<string, unknown> {
  return typeof raw === 'object' && raw !== null && !Array.isArray(raw);
}

function hasReqId(
  obj: Record<string, unknown>,
): obj is Record<string, unknown> & { reqId: number } {
  return typeof obj.reqId === 'number' && Number.isFinite(obj.reqId);
}

/** Garde la webview→host. Tout message ne correspondant pas exactement → undefined
 *  (le provider journalise et ignore ; ne jamais faire confiance à `raw: unknown`). */
export function parseBridgeMessage(raw: unknown): BridgeRequest | undefined {
  if (!isRecord(raw) || !hasReqId(raw)) {
    return undefined;
  }
  const reqId = raw.reqId;

  switch (raw.type) {
    case 'chat/send': {
      if (typeof raw.conversationId !== 'string' || typeof raw.message !== 'string') {
        return undefined;
      }
      // JSON strict : `agent` n'est posé que s'il est effectivement fourni (string).
      // Une clé absente OU explicitement undefined → champ absent du BridgeRequest.
      if (raw.agent === undefined) {
        return {
          kind: 'chat/send',
          reqId,
          conversationId: raw.conversationId,
          message: raw.message,
        };
      }
      if (typeof raw.agent !== 'string') {
        return undefined;
      }
      return {
        kind: 'chat/send',
        reqId,
        conversationId: raw.conversationId,
        message: raw.message,
        agent: raw.agent,
      };
    }
    case 'chat/cancel': {
      if (typeof raw.conversationId !== 'string') {
        return undefined;
      }
      return { kind: 'chat/cancel', reqId, conversationId: raw.conversationId };
    }
    case 'rpc': {
      if (typeof raw.method !== 'string') {
        return undefined;
      }
      return { kind: 'rpc', reqId, method: raw.method, params: raw.params };
    }
    default:
      return undefined;
  }
}

/** Erreur attrapée côté host → BridgeError. Forme réelle (packages/protocol
 *  connection.ts:23) : `RpcError { code: number (jsonrpc), message, vnlCode?: string }`.
 *  Propage `vnlCode` (l'identifiant VNL-RPC-XXX côté webview) ; code null sinon. */
export function mapRpcError(err: unknown): BridgeError {
  if (err instanceof RpcError) {
    return { code: err.vnlCode ?? null, message: err.message };
  }
  if (err instanceof Error) {
    return { code: null, message: err.message };
  }
  // Rejet d'une chaîne brute (ou autre) : message repris tel quel, code null.
  return { code: null, message: String(err) };
}

export interface BridgeApi {
  /** Appel RPC (le handle courant du superviseur ; l'appelant garantit handle présent
   *  ou lève lui-même). Doit pouvoir rejeter (RpcError). */
  request<T = unknown>(method: string, params?: unknown): Promise<T>;
  respond(resp: {
    type: 'chat/send/resp' | 'rpc/resp';
    reqId: number;
    ok: boolean;
    result?: unknown;
    error?: BridgeError;
  }): void;
  log(line: string): void;
}

/** Nom court du type d'un message webview, pour la journalisation d'un message rejeté
 *  (typeof / `.type` si présent — JAMAIS l'objet entier, il peut porter le message user). */
function describeMessage(raw: unknown): string {
  if (isRecord(raw) && typeof raw.type === 'string') {
    return `type=${raw.type}`;
  }
  return `typeof=${Array.isArray(raw) ? 'array' : typeof raw}`;
}

/** Traite UN message webview déjà reçu (raw) : parse → whitelist → relais → réponse.
 *  Ne rejette JAMAIS (erreurs RPC converties en réponses ok:false). `serverUp=false`
 *  → réponse immédiate VNL-EXT-021 (et log), sans toucher api.request. */
export async function handleBridgeRequest(
  raw: unknown,
  api: BridgeApi,
  serverUp: boolean,
): Promise<void> {
  const req = parseBridgeMessage(raw);
  if (!req) {
    // Pas de reqId fiable → aucune réponse possible ; on journalise et on ignore.
    api.log(`VNL-EXT-020: message webview ignoré (${describeMessage(raw)})`);
    return;
  }

  const type = req.kind === 'chat/send' ? 'chat/send/resp' : 'rpc/resp';
  const fail = (error: BridgeError): void => {
    api.respond({ type, reqId: req.reqId, ok: false, error });
  };

  // Whitelist : uniquement sur le canal générique {type:'rpc'}. 'chat/send'/'chat/cancel'
  // sont nommés, hors whitelist — forcés ici c'est -020 (pas un accident d'écriture).
  if (req.kind === 'rpc' && !RELAY_WHITELIST.includes(req.method)) {
    fail({ code: 'VNL-EXT-020', message: `méthode refusée par le pont : ${req.method}` });
    return;
  }

  if (!serverUp) {
    const error: BridgeError = {
      code: 'VNL-EXT-021',
      message: 'VNL-EXT-021: serveur vanyline non démarré',
    };
    api.log(error.message);
    fail(error);
    return;
  }

  try {
    let result: unknown;
    if (req.kind === 'chat/send') {
      // `agent` seulement s'il est défini : ne jamais envoyer agent:undefined en JSON.
      const params: Record<string, unknown> = {
        conversationId: req.conversationId,
        message: req.message,
      };
      if (req.agent !== undefined) {
        params.agent = req.agent;
      }
      result = await api.request('chat/send', params);
    } else if (req.kind === 'chat/cancel') {
      result = await api.request('chat/cancel', { conversationId: req.conversationId });
    } else {
      result = await api.request(req.method, req.params ?? {});
    }
    api.respond({ type, reqId: req.reqId, ok: true, result });
  } catch (err) {
    fail(mapRpcError(err));
  }
}
