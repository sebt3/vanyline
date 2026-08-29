import { describe, expect, it, vi, afterEach } from 'vitest';
import { RpcConnection, RpcError, RpcTimeoutError } from './connection';

function createMockTransport() {
  const listeners: Array<(line: string) => void> = [];
  const written: string[] = [];
  return {
    written,
    write(line: string) { written.push(line); },
    onLine(cb: (line: string) => void) { listeners.push(cb); },
    emit(line: string) { for (const cb of [...listeners]) cb(line); },
  };
}

describe('RpcConnection', () => {
  afterEach(() => {
    vi.restoreAllMocks();
    vi.clearAllTimers();
    vi.useRealTimers();
  });

  it("request('chat/send', params) writes a JSON-RPC request and resolves on response", async () => {
    const tr = createMockTransport();
    const conn = new RpcConnection(tr);

    const params = { conversationId: 'abc', message: 'hello' };
    const promise = conn.request('chat/send', params);

    // Vérifier que la requête a été écrite
    expect(tr.written.length).toBe(1);
    const _written = JSON.parse(tr.written[0]);
    expect(_written.jsonrpc).toBe('2.0');
    expect(_written.id).toBe(1);
    expect(_written.method).toBe('chat/send');
    expect(_written.params).toEqual(params);

    // Émettre une réponse
    tr.emit(JSON.stringify({ jsonrpc: '2.0', id: 1, result: { text: 'ok', toolCalls: [] } }));

    const result = await promise;
    expect(result).toEqual({ text: 'ok', toolCalls: [] });

    conn.close();
  });

  it('request without params omits the params field', async () => {
    const tr = createMockTransport();
    const conn = new RpcConnection(tr);

    const promise = conn.request('shutdown');
    expect(tr.written.length).toBe(1);

    const parsed = JSON.parse(tr.written![0]);
    expect(parsed.jsonrpc).toBe('2.0');
    expect(parsed.id).toBe(1);
    expect(parsed.method).toBe('shutdown');
    expect('params' in parsed).toBe(false);

    tr.emit(JSON.stringify({ jsonrpc: '2.0', id: 1, result: null }));
    const result = await promise;
    expect(result).toBe(null);

    conn.close();
  });

  it('request rejects with RpcError on server error response', async () => {
    const tr = createMockTransport();
    const conn = new RpcConnection(tr);

    const promise = conn.request('nonexistent');

    tr.emit(JSON.stringify({
      jsonrpc: '2.0',
      id: 1,
      error: { code: -32601, message: 'Method not found: x', data: { code: 'VNL-RPC-004' } },
}));

    // verifier la rejection
    await expect(promise).rejects.toBeInstanceOf(RpcError);
    await expect(promise).rejects.toThrow(/Method not found: x/);

    // Proprietes custom — la promise est déjà rejectée, le .catch() est synchrone
    const props = (await promise.then(
      null,
      (e: unknown) => e as RpcError,
    )) as RpcError;
    expect(props.code).toBe(-32601);
    expect(props.message).toBe('Method not found: x');
    expect(props.vnlCode).toBe('VNL-RPC-004');

    conn.close();
  });

  it('multi-request correlation: responses out of order resolve correct promises', async () => {
    const tr = createMockTransport();
    const conn = new RpcConnection(tr);

    const a = conn.request('methodA', { key: 'a' });
    const b = conn.request('methodB', { key: 'b' });

    // Réponses dans l'ordre inversé : b d'abord, puis a
    tr.emit(JSON.stringify({ jsonrpc: '2.0', id: 2, result: { from: 'b' } }));
    tr.emit(JSON.stringify({ jsonrpc: '2.0', id: 1, result: { from: 'a' } }));

    const [resultA, resultB] = await Promise.all([a, b]);
    expect(resultA).toEqual({ from: 'a' });
    expect(resultB).toEqual({ from: 'b' });

    conn.close();
  });

  it('notification dispatch calls registered handler', async () => {
    const tr = createMockTransport();
    const conn = new RpcConnection(tr);

    const handler = vi.fn();
    conn.onNotification('chat/event', handler);

    tr.emit(JSON.stringify({
      jsonrpc: '2.0',
      method: 'chat/event',
      params: { conversationId: 'c1', seq: 1, event: { type: 'token', content: 'ok' } },
    }));

    expect(handler).toHaveBeenCalledWith({
      conversationId: 'c1',
      seq: 1,
      event: { type: 'token', content: 'ok' },
    });

    // Deuxième notification appellera à nouveau
    tr.emit(JSON.stringify({
      jsonrpc: '2.0',
      method: 'chat/event',
      params: { conversationId: 'c1', seq: 2, event: { type: 'done' } },
    }));

    expect(handler).toHaveBeenCalledTimes(2);

    conn.close();
  });

  it('notification without handler is silently ignored', () => {
    const tr = createMockTransport();
    const conn = new RpcConnection(tr);

    // Pas de handler enregistré pour 'unknown/notif'
    expect(() => {
      tr.emit(JSON.stringify({
        jsonrpc: '2.0',
        method: 'unknown/notif',
        params: { foo: 'bar' },
      }));
    }).not.toThrow();

    conn.close();
  });

  it('close() rejects pending requests and ignores subsequent lines', async () => {
    const tr = createMockTransport();
    const conn = new RpcConnection(tr, { timeoutMs: 5000 });

    const promise = conn.request('chat/send', { conversationId: 'x', message: 'hi' });

    // Fermer la connection
    conn.close();

    // La request en attente doit rejeter avec RpcTimeoutError
    await expect(promise).rejects.toBeInstanceOf(RpcTimeoutError);

    // Propriétés custom — la promise est déjà rejectée, le .catch() est synchrone
    const caught = await promise.then(null, (e: unknown) => e as RpcTimeoutError);
    expect(caught).toBeInstanceOf(RpcTimeoutError);

    // Emettre des lignes apres close — doit être ignoré
    expect(() => {
      tr.emit(JSON.stringify({ jsonrpc: '2.0', id: 1, result: { text: 'late' } }));
      tr.emit(JSON.stringify({ jsonrpc: '2.0', method: 'chat/event', params: {} }));
    }).not.toThrow();
  });

  it('timeout rejects with RpcTimeoutError (fake timers)', () => {
    const origSetTimeout = globalThis.setTimeout;
    let timeoutFn: { fn: () => void; timer: number } | null = null;

    globalThis.setTimeout = ((cb: () => void, ms: number) => {
      // Simuler un timer qui execute immediatement
      cb();
      const timer = Math.random() * 1000 | 0;
      return timer as unknown as ReturnType<typeof origSetTimeout>;
    }) as unknown as typeof globalThis.setTimeout;

    const tr = createMockTransport();
    const conn = new RpcConnection(tr, { timeoutMs: 30 });

    const promise = conn.request('chat/send', { conversationId: 'x', message: 'hi' });

    // Le timer a execute immediatement, donc la rejection est immediate
    expect(promise).rejects.toBeInstanceOf(RpcTimeoutError);

    // Remettre setTimeout original
    globalThis.setTimeout = origSetTimeout;
  });
});