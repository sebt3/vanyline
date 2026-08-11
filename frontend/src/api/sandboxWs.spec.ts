import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { SandboxFsClient, openSandboxWs } from './sandboxWs';

class FakeWebSocket {
  url: string;
  sent: string[] = [];
  private listeners: Record<string, Array<(ev: { data: string }) => void>> = {};
  constructor(url: string) {
    this.url = url;
  }
  addEventListener(type: string, cb: (ev: { data: string }) => void) {
    (this.listeners[type] ??= []).push(cb);
  }
  removeEventListener(type: string, cb: (ev: { data: string }) => void) {
    this.listeners[type] = (this.listeners[type] ?? []).filter((f) => f !== cb);
  }
  send(data: string) {
    this.sent.push(data);
  }
  emitMessage(data: string) {
    for (const cb of [...(this.listeners['message'] ?? [])]) cb({ data });
  }
}

/** La file `queue: Promise.resolve()` de `SandboxFsClient` programme
 *  `run` via `.then()`, donc l'exécution des effets secondaires
 *  (addEventListener + send) est différée à la prochaine microtask. */
async function flushQueue() {
  await Promise.resolve();
}

describe('openSandboxWs', () => {
  let fetchSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    vi.stubGlobal('WebSocket', FakeWebSocket);
    fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy.mockClear();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('mine le ticket et ouvre wss://<wsHost><path>?ticket=...', async () => {
    (fetchSpy as any).mockResolvedValue(
      new Response(
        JSON.stringify({ ticket: 'abc', wsHost: 'my-sandbox.sandboxes.example.com' }),
        { status: 200 },
      ),
    );

    const ws = await openSandboxWs('my-sandbox', '/ws/fs');

    expect(fetchSpy).toHaveBeenCalledWith(
      '/api/sandboxes/my-sandbox/ws-ticket',
      expect.any(Object),
    );
    expect((ws as any).url).toBe(
      'wss://my-sandbox.sandboxes.example.com/ws/fs?ticket=abc',
    );
  });

  it('mine le ticket et ouvre wss://<wsHost><path>?ticket=... (terminal)', async () => {
    (fetchSpy as any).mockResolvedValue(
      new Response(
        JSON.stringify({ ticket: 'xyz', wsHost: 'my-sandbox.sandboxes.example.com' }),
        { status: 200 },
      ),
    );

    const ws = await openSandboxWs('my-sandbox', '/ws/terminal');

    expect((ws as any).url).toBe(
      'wss://my-sandbox.sandboxes.example.com/ws/terminal?ticket=xyz',
    );
  });
});

describe('SandboxFsClient', () => {
  let ws: FakeWebSocket;

  beforeEach(() => {
    vi.stubGlobal('WebSocket', FakeWebSocket);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('envoie {op, ...params} et résout la réponse ok', async () => {
    ws = new FakeWebSocket('wss://example.com/ws/fs');
    const client = new SandboxFsClient(ws as unknown as WebSocket);

    const p = client.request('list', { path: '.' });

    // Flushed la microtask programmée par queue.then(run, run)
    await flushQueue();

    expect(ws.sent[0]).toBe('{"op":"list","path":"."}');
    ws.emitMessage('{"ok":true,"entries":"a.txt"}');

    const result = await p;
    expect(result).toEqual({ ok: true, entries: 'a.txt' });
  });

  it('rejette quand ok est false', async () => {
    ws = new FakeWebSocket('wss://example.com/ws/fs');
    const client = new SandboxFsClient(ws as unknown as WebSocket);

    const p = client.request('read', { path: 'missing.txt' });

    await flushQueue();
    ws.emitMessage('{"ok":false,"error":"boom"}');

    await expect(p).rejects.toThrow('boom');
  });

  it('les requêtes sont sérialisées : la seconde attend la réponse de la première', async () => {
    ws = new FakeWebSocket('wss://example.com/ws/fs');
    const client = new SandboxFsClient(ws as unknown as WebSocket);

    const p1 = client.request('read', { path: 'a.txt' });
    const p2 = client.request('read', { path: 'b.txt' });

    // Aucune réponse n'ayant été émise, une seule requête a été envoyée
    await flushQueue();
    expect(ws.sent.length).toBe(1);

    // Après la première réponse, la seconde requête doit être envoyée
    ws.emitMessage('{"ok":true,"content":"aaa"}');
    await flushQueue();
    expect(ws.sent.length).toBe(2);
    expect(ws.sent[1]).toBe('{"op":"read","path":"b.txt"}');

    const r1 = await p1;
    expect(r1).toEqual({ ok: true, content: 'aaa' });
    p2.catch(() => null); // sérialisation vérifiée : p2 programmé dans la queue
  });
});
