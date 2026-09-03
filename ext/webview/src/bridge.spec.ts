// @vitest-environment jsdom
import { describe, expect, it, vi } from 'vitest';
import type { UIMessage, UIMessageChunk } from 'ai';
import {
  BridgeRpcError,
  createBridgeClient,
  getBridgeClient,
  resetBridgeSingleton,
  type BridgeDeps,
} from './bridge';
import { PostMessageChatTransport } from './postMessageChatTransport';
import { createBridgeBackend } from './backend';

/** Harness du contrat 04b : post → posted[], listen → listeners (unsubscribe retirée). */
function harness(): {
  posted: Record<string, unknown>[];
  emit: (msg: unknown) => void;
  /** Vide les microtâches ET les jobs internes des streams (macrotâche). */
  flush: () => Promise<void>;
  clientDeps: BridgeDeps;
} {
  const posted: Record<string, unknown>[] = [];
  const listeners: Array<(data: unknown) => void> = [];
  return {
    posted,
    emit: (msg: unknown) => {
      for (const cb of [...listeners]) cb(msg);
    },
    flush: async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    },
    clientDeps: {
      post: (m: unknown) => {
        posted.push(m as Record<string, unknown>);
      },
      listen: (cb: (data: unknown) => void) => {
        listeners.push(cb);
        return () => {
          const i = listeners.indexOf(cb);
          if (i >= 0) listeners.splice(i, 1);
        };
      },
    },
  };
}

describe('createBridgeClient — request (rpc)', () => {
  it('cas 1 — posted {type:rpc, reqId, method, params}, résout le résultat, reqId incrémenté', async () => {
    const h = harness();
    const client = createBridgeClient(h.clientDeps);

    const p = client.request('conversations/list', {});
    expect(h.posted).toHaveLength(1);
    expect(h.posted[0]).toMatchObject({ type: 'rpc', method: 'conversations/list', params: {} });
    const reqId = h.posted[0]?.reqId as number;
    expect(typeof reqId).toBe('number');

    h.emit({ type: 'rpc/resp', reqId, ok: true, result: { a: 1 } });
    await expect(p).resolves.toEqual({ a: 1 });

    const p2 = client.request('config/agents', {});
    const reqId2 = h.posted[1]?.reqId as number;
    expect(reqId2).toBeGreaterThan(reqId);
    h.emit({ type: 'rpc/resp', reqId: reqId2, ok: true, result: [] });
    await expect(p2).resolves.toEqual([]);
  });

  it('cas 2 — ok:false → rejette BridgeRpcError (code et message préservés, instanceof)', async () => {
    const h = harness();
    const client = createBridgeClient(h.clientDeps);

    const p = client.request('config/agents', {});
    const reqId = h.posted[0]?.reqId as number;
    h.emit({
      type: 'rpc/resp',
      reqId,
      ok: false,
      error: { code: 'VNL-EXT-021', message: 'x' },
    });

    const err = await p.then(
      () => null,
      (e: unknown) => e,
    );
    expect(err).toBeInstanceOf(BridgeRpcError);
    expect(err).toMatchObject({ code: 'VNL-EXT-021', message: 'x' });
  });

  it('cas 3 — réponse dont le reqId est inconnu → silencieusement ignorée', async () => {
    const h = harness();
    const client = createBridgeClient(h.clientDeps);

    let settled = false;
    void client.request('conversations/list', {}).then(
      () => {
        settled = true;
      },
      () => {
        settled = true;
      },
    );

    // Réponse tardive (reqId d'une vie précédente du pont), reqId absent, non-objet.
    h.emit({ type: 'rpc/resp', reqId: 9999, ok: true, result: 'tardive' });
    h.emit({ type: 'rpc/resp', ok: true, result: 'sans-reqId' });
    h.emit('chaîne');
    h.emit(null);
    await h.flush();
    expect(settled).toBe(false);
  });
});

describe('createBridgeClient — messages nommés et abonnements', () => {
  it('cas 4 — chatSend sans agent → SANS la clé agent ; avec agent → présente ; corrélation chat/send/resp', async () => {
    const h = harness();
    const client = createBridgeClient(h.clientDeps);

    const p1 = client.chatSend({ conversationId: 'c1', message: 'bonjour' });
    const sent1 = h.posted[0] as Record<string, unknown>;
    expect(sent1).toMatchObject({ type: 'chat/send', conversationId: 'c1', message: 'bonjour' });
    expect(Object.hasOwn(sent1, 'agent')).toBe(false);
    h.emit({ type: 'chat/send/resp', reqId: sent1['reqId'], ok: true, result: { text: 'hi' } });
    await expect(p1).resolves.toEqual({ text: 'hi' });

    const p2 = client.chatSend({ conversationId: 'c1', message: 'salut', agent: 'orchestrator' });
    const sent2 = h.posted[1] as Record<string, unknown>;
    expect(sent2).toMatchObject({
      type: 'chat/send',
      conversationId: 'c1',
      message: 'salut',
      agent: 'orchestrator',
    });
    h.emit({ type: 'chat/send/resp', reqId: sent2['reqId'], ok: true, result: null });
    await expect(p2).resolves.toBeNull();
  });

  it('cas 5 — chatCancel → posted {type:chat/cancel, reqId, conversationId}, corrélé sur rpc/resp', async () => {
    const h = harness();
    const client = createBridgeClient(h.clientDeps);

    const p = client.chatCancel('c1');
    const sent = h.posted[0] as Record<string, unknown>;
    expect(sent).toMatchObject({ type: 'chat/cancel', conversationId: 'c1' });
    expect(typeof sent['reqId']).toBe('number');

    h.emit({ type: 'rpc/resp', reqId: sent['reqId'], ok: true, result: null });
    await expect(p).resolves.toBeNull();
  });

  it('cas 6 — onChatEvent : reçoit {conversationId, seq, event}, unsubscribe → plus de cb', () => {
    const h = harness();
    const client = createBridgeClient(h.clientDeps);

    const seen: unknown[] = [];
    const off = client.onChatEvent((p) => {
      seen.push(p);
    });

    h.emit({
      type: 'chat/event',
      conversationId: 'c1',
      seq: 0,
      event: { type: 'token', content: 'ok' },
    });
    expect(seen).toEqual([
      { conversationId: 'c1', seq: 0, event: { type: 'token', content: 'ok' } },
    ]);

    off();
    h.emit({ type: 'chat/event', conversationId: 'c1', seq: 1, event: { type: 'done' } });
    expect(seen).toHaveLength(1);
  });

  it('cas 7 — onMessage session/pick : conversationId null → cb(null) ; autres types ignorés', () => {
    const h = harness();
    const client = createBridgeClient(h.clientDeps);

    const picks: Array<string | null> = [];
    const off = client.onMessage('session/pick', (id) => {
      picks.push(id);
    });

    h.emit({ type: 'session/pick', conversationId: null });
    h.emit({ type: 'session/pick', conversationId: 'abc' });
    expect(picks).toEqual([null, 'abc']);

    // Autres types → ignorés par cet abonnement.
    h.emit({ type: 'session/new', conversationId: 'zzz' });
    h.emit({ type: 'inconnu', conversationId: 'q' });
    expect(picks).toHaveLength(2);

    off();
    h.emit({ type: 'session/pick', conversationId: 'def' });
    expect(picks).toHaveLength(2);
  });
});

describe('getBridgeClient — singleton de production', () => {
  it('mémoïsé au premier appel (acquireVsCodeApi appelé une fois), resetBridgeSingleton repart à zéro', () => {
    const acquire = vi.fn(() => ({ postMessage: vi.fn() }));
    window.acquireVsCodeApi = acquire;

    resetBridgeSingleton();
    const a = getBridgeClient();
    const b = getBridgeClient();
    expect(a).toBe(b);
    expect(acquire).toHaveBeenCalledTimes(1);

    resetBridgeSingleton();
    const c = getBridgeClient();
    expect(c).not.toBe(a);
    expect(acquire).toHaveBeenCalledTimes(2);
    resetBridgeSingleton();
  });
});

function userMessage(text: string): UIMessage {
  return { id: 'u1', role: 'user', parts: [{ type: 'text', text }] };
}

/** Lit le stream jusqu'à sa fermeture et collecte les chunks. */
async function collectChunks(
  stream: ReadableStream<UIMessageChunk>,
): Promise<UIMessageChunk[]> {
  const reader = stream.getReader();
  const chunks: UIMessageChunk[] = [];
  for (;;) {
    const { value, done } = await reader.read();
    if (done) break;
    chunks.push(value);
  }
  return chunks;
}

describe('PostMessageChatTransport', () => {
  function transportHarness(getAgent: () => string | undefined = () => undefined) {
    const h = harness();
    const bridge = createBridgeClient(h.clientDeps);
    const transport = new PostMessageChatTransport(bridge, getAgent);
    const sentChat = (): Record<string, unknown> =>
      h.posted.find((m) => m.type === 'chat/send') as Record<string, unknown>;
    return { ...h, transport, sentChat };
  }

  it('cas 8 — sendMessages poste chat/send ; token+done → chunks dont un text-delta, puis fermeture', async () => {
    const h = transportHarness();
    const stream = await h.transport.sendMessages({
      chatId: 'c1',
      messages: [userMessage('bonjour')],
      abortSignal: undefined,
    });

    const sent = h.sentChat();
    expect(sent).toMatchObject({ conversationId: 'c1', message: 'bonjour' });
    expect(Object.hasOwn(sent, 'agent')).toBe(false);

    const chunksPromise = collectChunks(stream);
    h.emit({
      type: 'chat/event',
      conversationId: 'c1',
      seq: 0,
      event: { type: 'token', content: 'ok' },
    });
    h.emit({ type: 'chat/event', conversationId: 'c1', seq: 1, event: { type: 'done' } });

    // collectChunks retourne uniquement quand le reader est `done` → stream fermé.
    const chunks = await chunksPromise;
    expect(chunks.some((c) => c.type === 'text-delta' && c.delta === 'ok')).toBe(true);
  });

  it('cas 9 — événements d\'autres conversations ignorés par le stream', async () => {
    const h = transportHarness();
    const stream = await h.transport.sendMessages({
      chatId: 'c1',
      messages: [userMessage('bonjour')],
      abortSignal: undefined,
    });

    const chunksPromise = collectChunks(stream);
    h.emit({
      type: 'chat/event',
      conversationId: 'c1',
      seq: 0,
      event: { type: 'token', content: 'ok' },
    });
    h.emit({
      type: 'chat/event',
      conversationId: 'autre',
      seq: 0,
      event: { type: 'token', content: 'IGNORE' },
    });
    h.emit({ type: 'chat/event', conversationId: 'c1', seq: 1, event: { type: 'done' } });

    const chunks = await chunksPromise;
    const text = chunks
      .filter((c) => c.type === 'text-delta')
      .map((c) => (c.type === 'text-delta' ? c.delta : ''))
      .join('');
    expect(text).toBe('ok');
  });

  it('cas 10 — chat/send/resp ok:false avant done → l\'erreur atteint le lecteur avec « busy »', async () => {
    // Comportement figé de chatEventsToUIStream (packages/ui, non modifiable) : une
    // erreur de la source est convertie en chunk {type:'error', errorText} puis le
    // stream se ferme — la lecture ne rejette pas côté reader, c'est le chunk qui
    // porte l'erreur ('busy' via BridgeRpcError.message). Assertion équivalente.
    const h = transportHarness();
    const stream = await h.transport.sendMessages({
      chatId: 'c1',
      messages: [userMessage('bonjour')],
      abortSignal: undefined,
    });

    const chunksPromise = collectChunks(stream);
    const sent = h.sentChat();
    h.emit({
      type: 'chat/send/resp',
      reqId: sent['reqId'],
      ok: false,
      error: { code: 'VNL-RPC-002', message: 'busy' },
    });

    const chunks = await chunksPromise;
    const errChunk = chunks.find((c) => c.type === 'error');
    expect(errChunk).toBeDefined();
    expect(errChunk?.type === 'error' ? errChunk.errorText : '').toContain('busy');
  });

  it('cas 11 — reader.cancel() du stream → chat/cancel posté pour la conversation', async () => {
    const h = transportHarness();
    const stream = await h.transport.sendMessages({
      chatId: 'c1',
      messages: [userMessage('bonjour')],
      abortSignal: undefined,
    });

    const reader = stream.getReader();
    await reader.cancel();
    await h.flush();

    expect(
      h.posted.some((m) => m.type === 'chat/cancel' && m.conversationId === 'c1'),
    ).toBe(true);
  });

  it('cas 11b — issue terminale → listener « abort » démonté (pas de chat/cancel tardif)', async () => {
    // Fuite corrigée : sur rejet du chatSend (comme sur done/error), le listener
    // 'abort' de l'abortSignal doit être retiré — sinon un abort postérieur au tour
    // relance un chat/cancel parasite dans une webview longue-vie.
    const ac = new AbortController();
    const h = transportHarness();
    const stream = await h.transport.sendMessages({
      chatId: 'c1',
      messages: [userMessage('bonjour')],
      abortSignal: ac.signal,
    });

    const chunksPromise = collectChunks(stream);
    h.emit({
      type: 'chat/send/resp',
      reqId: h.sentChat()['reqId'],
      ok: false,
      error: { code: 'VNL-RPC-002', message: 'busy' },
    });
    await chunksPromise;

    ac.abort();
    await h.flush();
    expect(h.posted.some((m) => m.type === 'chat/cancel')).toBe(false);
  });

  it('cas 11c — done normal → listener « abort » démonté', async () => {
    const ac = new AbortController();
    const h = transportHarness();
    const stream = await h.transport.sendMessages({
      chatId: 'c1',
      messages: [userMessage('bonjour')],
      abortSignal: ac.signal,
    });

    const chunksPromise = collectChunks(stream);
    h.emit({ type: 'chat/event', conversationId: 'c1', seq: 0, event: { type: 'done' } });
    await chunksPromise;

    ac.abort();
    await h.flush();
    expect(h.posted.some((m) => m.type === 'chat/cancel')).toBe(false);
  });
});

describe('createBridgeBackend', () => {
  function backendHarness() {
    const h = harness();
    const bridge = createBridgeClient(h.clientDeps);
    const backend = createBridgeBackend(bridge);
    const lastReq = (): Record<string, unknown> =>
      h.posted[h.posted.length - 1] as Record<string, unknown>;
    const respond = (result: unknown): void => {
      h.emit({ type: 'rpc/resp', reqId: lastReq()?.['reqId'], ok: true, result });
    };
    return { ...h, backend, lastReq, respond };
  }

  it('cas 12 — listConversations : repli titre « Session <id8> », createdAt \'\', ordre et titres conservés', async () => {
    const h = backendHarness();
    const p = h.backend.listConversations();
    expect(h.lastReq()).toMatchObject({ type: 'rpc', method: 'conversations/list', params: {} });

    h.respond([
      { id: 'abcdef0123456789', messageCount: 2 },
      { id: 'zz', title: 'Mon titre', messageCount: 0 },
    ]);
    await expect(p).resolves.toEqual([
      { id: 'abcdef0123456789', title: 'Session abcdef01', createdAt: '' },
      { id: 'zz', title: 'Mon titre', createdAt: '' },
    ]);
  });

  it('cas 13 — loadMessages : filtre user/assistant, id = index filtré, contenus intacts', async () => {
    const h = backendHarness();
    const p = h.backend.loadMessages('conv1');
    expect(h.lastReq()).toMatchObject({ method: 'conversations/get', params: { id: 'conv1' } });

    h.respond({
      id: 'conv1',
      messages: [
        { role: 'user', content: 'a' },
        { role: 'system', content: 'ignoré' },
        { role: 'assistant', content: 'b' },
        { role: 'user', content: 'c' },
      ],
    });
    await expect(p).resolves.toEqual([
      { id: '0', role: 'user', content: 'a' },
      { id: '1', role: 'assistant', content: 'b' },
      { id: '2', role: 'user', content: 'c' },
    ]);
  });

  it('cas 14 — createConversation : conversations/create {} → id', async () => {
    const h = backendHarness();
    const p = h.backend.createConversation();
    expect(h.lastReq()).toMatchObject({ method: 'conversations/create', params: {} });
    h.respond({ id: 'x' });
    await expect(p).resolves.toBe('x');
  });
});
