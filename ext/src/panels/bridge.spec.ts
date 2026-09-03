import { describe, expect, it, vi } from 'vitest';
import { RpcError } from '@vanyline/protocol';
import {
  RELAY_WHITELIST,
  handleBridgeRequest,
  mapRpcError,
  parseBridgeMessage,
  type BridgeApi,
} from './bridge';

/** Réponse enregistrée : paramètre exact de BridgeApi.respond (pas de re-déclaration). */
type Resp = Parameters<BridgeApi['respond']>[0];

/** API factice : enregistre request/respond/log, request résout {} par défaut. */
function fakeApi(requestImpl?: (method: string, params?: unknown) => Promise<unknown>) {
  const responses: Resp[] = [];
  const logged: string[] = [];
  const request = vi.fn(async (method: string, params?: unknown): Promise<unknown> =>
    requestImpl ? requestImpl(method, params) : {},
  );
  const api: BridgeApi = {
    // le mock concret n'est pas générique (<T>) — pont assumé côté test
    request: request as BridgeApi['request'],
    respond: (resp) => {
      responses.push(resp);
    },
    log: (line) => {
      logged.push(line);
    },
  };
  return { api, request, responses, logged };
}

describe('parseBridgeMessage', () => {
  it('cas 1 — chat/send complet → BridgeRequest', () => {
    expect(
      parseBridgeMessage({
        type: 'chat/send',
        reqId: 7,
        conversationId: 'c1',
        message: 'bonjour',
        agent: 'qwen',
      }),
    ).toEqual({ kind: 'chat/send', reqId: 7, conversationId: 'c1', message: 'bonjour', agent: 'qwen' });
  });

  it('cas 1 — chat/send sans agent → la clé agent est absente (pas undefined)', () => {
    const req = parseBridgeMessage({ type: 'chat/send', reqId: 1, conversationId: 'c1', message: 'x' });
    expect(req).toEqual({ kind: 'chat/send', reqId: 1, conversationId: 'c1', message: 'x' });
    if (req) {
      expect('agent' in req).toBe(false);
    }
  });

  it("cas 1 — chat/cancel et rpc valides parsent aussi", () => {
    expect(parseBridgeMessage({ type: 'chat/cancel', reqId: 2, conversationId: 'c1' })).toEqual({
      kind: 'chat/cancel',
      reqId: 2,
      conversationId: 'c1',
    });
    expect(parseBridgeMessage({ type: 'rpc', reqId: 3, method: 'config/agents', params: { a: 1 } })).toEqual({
      kind: 'rpc',
      reqId: 3,
      method: 'config/agents',
      params: { a: 1 },
    });
  });

  it('cas 1 — tout le reste → undefined (jamais faire confiance à raw)', () => {
    // reqId non-numérique
    expect(parseBridgeMessage({ type: 'chat/send', reqId: '1', conversationId: 'c', message: 'm' })).toBeUndefined();
    expect(parseBridgeMessage({ type: 'chat/send', conversationId: 'c', message: 'm' })).toBeUndefined();
    // conversationId non-string
    expect(parseBridgeMessage({ type: 'chat/send', reqId: 1, conversationId: 42, message: 'm' })).toBeUndefined();
    // type inconnu / absent, null, tableau
    expect(parseBridgeMessage({ type: 'inconnu', reqId: 1 })).toBeUndefined();
    expect(parseBridgeMessage(null)).toBeUndefined();
    expect(parseBridgeMessage([])).toBeUndefined();
  });
});

describe('mapRpcError', () => {
  it('cas 2 — RpcError : propage vnlCode (identifiant VNL-RPC-* côté webview)', () => {
    expect(mapRpcError(new RpcError(-32000, 'busy', 'VNL-RPC-002'))).toEqual({
      code: 'VNL-RPC-002',
      message: 'busy',
    });
    expect(mapRpcError(new RpcError(-32601, 'not found'))).toEqual({ code: null, message: 'not found' });
  });

  it("cas 2 — Error simple et chaîne brute → code null, message repris", () => {
    expect(mapRpcError(new Error('x'))).toEqual({ code: null, message: 'x' });
    expect(mapRpcError('chaîne')).toEqual({ code: null, message: 'chaîne' });
  });
});

describe('handleBridgeRequest', () => {
  it('cas 3 — chat/send valide relaie chat/send SANS la clé agent quand non fournie', async () => {
    const h = fakeApi(async () => ({ text: 'ok', toolCalls: [] }));
    await handleBridgeRequest(
      { type: 'chat/send', reqId: 11, conversationId: 'c1', message: 'bonjour' },
      h.api,
      true,
    );
    expect(h.request).toHaveBeenCalledTimes(1);
    const [method, params] = h.request.mock.calls[0];
    expect(method).toBe('chat/send');
    expect(params).toEqual({ conversationId: 'c1', message: 'bonjour' });
    // JSON strict : la clé doit être ABSENTE, pas undefined (toEqual seul ne suffit pas).
    expect('agent' in (params as object)).toBe(false);
    expect(h.responses).toEqual([
      { type: 'chat/send/resp', reqId: 11, ok: true, result: { text: 'ok', toolCalls: [] } },
    ]);
  });

  it('cas 3 — agent fourni → présent dans les params RPC', async () => {
    const h = fakeApi();
    await handleBridgeRequest(
      { type: 'chat/send', reqId: 12, conversationId: 'c1', message: 'x', agent: 'qwen' },
      h.api,
      true,
    );
    expect(h.request.mock.calls[0]).toEqual(['chat/send', { conversationId: 'c1', message: 'x', agent: 'qwen' }]);
  });

  it('cas 4 — rejet RpcError → réponse ok:false avec le code VNL-RPC (chat/send/resp)', async () => {
    const h = fakeApi(async () => {
      throw new RpcError(-32000, 'busy', 'VNL-RPC-002');
    });
    await handleBridgeRequest({ type: 'chat/send', reqId: 21, conversationId: 'c1', message: 'm' }, h.api, true);
    expect(h.responses).toEqual([
      { type: 'chat/send/resp', reqId: 21, ok: false, error: { code: 'VNL-RPC-002', message: 'busy' } },
    ]);
  });

  it("cas 4 — rejet sur un relais rpc → rpc/resp (le type suit le message reçu)", async () => {
    const h = fakeApi(async () => {
      throw new RpcError(-32000, 'busy', 'VNL-RPC-002');
    });
    await handleBridgeRequest({ type: 'rpc', reqId: 22, method: 'conversations/list' }, h.api, true);
    expect(h.responses).toEqual([
      { type: 'rpc/resp', reqId: 22, ok: false, error: { code: 'VNL-RPC-002', message: 'busy' } },
    ]);
  });

  it('cas 5 — méthode whitelistée passe : request(method, params ?? {})', async () => {
    const h = fakeApi(async () => ['a']);
    await handleBridgeRequest({ type: 'rpc', reqId: 31, method: 'conversations/list' }, h.api, true);
    expect(h.request).toHaveBeenCalledWith('conversations/list', {});
    expect(h.responses).toEqual([{ type: 'rpc/resp', reqId: 31, ok: true, result: ['a'] }]);
  });

  it('cas 5 — méthode hors whitelist → VNL-EXT-020 sans appel request', async () => {
    const h = fakeApi();
    await handleBridgeRequest({ type: 'rpc', reqId: 32, method: 'config/providers/test' }, h.api, true);
    expect(h.request).not.toHaveBeenCalled();
    expect(h.responses).toHaveLength(1);
    expect(h.responses[0].type).toBe('rpc/resp');
    expect(h.responses[0].ok).toBe(false);
    expect(h.responses[0].error?.code).toBe('VNL-EXT-020');
    expect(h.responses[0].error?.message).toContain('config/providers/test');
  });

  it("cas 5 — chat/send forcé via {type:'rpc'} → VNL-EXT-020 (messages nommés uniquement)", async () => {
    const h = fakeApi();
    await handleBridgeRequest(
      { type: 'rpc', reqId: 33, method: 'chat/send', params: { conversationId: 'c', message: 'm' } },
      h.api,
      true,
    );
    expect(h.request).not.toHaveBeenCalled();
    expect(h.responses).toHaveLength(1);
    expect(h.responses[0].error?.code).toBe('VNL-EXT-020');
    // la whitelist est le contrat de sécurité — gélée ici, exacte :
    expect(RELAY_WHITELIST).toEqual([
      'conversations/list',
      'conversations/get',
      'conversations/create',
      'conversations/delete',
      'config/agents',
    ]);
  });

  it('cas 6 — serverUp=false → VNL-EXT-021 du bon type de réponse, request jamais appelé', async () => {
    const h = fakeApi();
    await handleBridgeRequest({ type: 'chat/send', reqId: 41, conversationId: 'c1', message: 'm' }, h.api, false);
    await handleBridgeRequest({ type: 'rpc', reqId: 42, method: 'conversations/list' }, h.api, false);
    expect(h.request).not.toHaveBeenCalled();
    expect(h.responses).toEqual([
      {
        type: 'chat/send/resp',
        reqId: 41,
        ok: false,
        error: { code: 'VNL-EXT-021', message: expect.stringContaining('VNL-EXT-021') },
      },
      {
        type: 'rpc/resp',
        reqId: 42,
        ok: false,
        error: { code: 'VNL-EXT-021', message: expect.stringContaining('VNL-EXT-021') },
      },
    ]);
    expect(h.logged.length).toBeGreaterThan(0);
  });

  it('cas 7 — message invalide : journalisé (sans dump de l\'objet), AUCUNE réponse', async () => {
    const h = fakeApi();
    await handleBridgeRequest(null, h.api, true);
    await handleBridgeRequest({ type: 'inconnu', reqId: 51 }, h.api, true);
    expect(h.logged).toHaveLength(2);
    expect(h.logged[0]).toContain('object'); // typeof cité, jamais l'objet entier
    expect(h.logged[1]).toContain('inconnu');
    expect(h.responses).toEqual([]);
    expect(h.request).not.toHaveBeenCalled();
  });

  it('cas 8 — ne rejette jamais, même sur rejet d\'une chaîne brute (code null)', async () => {
    const h = fakeApi(async () => {
      throw 'panne brute';
    });
    await expect(
      handleBridgeRequest({ type: 'chat/send', reqId: 61, conversationId: 'c1', message: 'm' }, h.api, true),
    ).resolves.toBeUndefined();
    expect(h.responses).toEqual([
      { type: 'chat/send/resp', reqId: 61, ok: false, error: { code: null, message: 'panne brute' } },
    ]);
  });
});
