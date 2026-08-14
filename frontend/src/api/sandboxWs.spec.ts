import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { SandboxFsClient, openSandboxWs } from './sandboxWs';

class FakeWebSocket {
  static instances: FakeWebSocket[] = [];
  url: string;
  sent: string[] = [];
  private listeners: Record<string, Array<(ev: { data?: string; type?: string }) => void>> = {};
  constructor(url: string) {
    this.url = url;
    FakeWebSocket.instances.push(this);
  }
  addEventListener(type: string, cb: (ev: { data?: string; type?: string }) => void) {
    (this.listeners[type] ??= []).push(cb);
  }
  removeEventListener(type: string, cb: (ev: { data?: string; type?: string }) => void) {
    this.listeners[type] = (this.listeners[type] ?? []).filter((f) => f !== cb);
  }
  send(data: string) {
    this.sent.push(data);
  }
  emitMessage(data: string) {
    for (const cb of [...(this.listeners['message'] ?? [])]) cb({ data });
  }
  emitOpen() {
    for (const cb of [...(this.listeners['open'] ?? [])]) cb({ type: 'open' });
  }
  emitError() {
    for (const cb of [...(this.listeners['error'] ?? [])]) cb({ type: 'error' });
  }
}

/** Laisse la chaîne `client.post(...)` (plusieurs `await` internes : fetch,
 *  response.json()) construire le WebSocket avant qu'on aille chercher
 *  l'instance dans `FakeWebSocket.instances`. Un macrotask flush plutôt
 *  qu'un nombre fixe de microtasks — robuste au nombre réel d'`await`
 *  traversés dans `client.ts`. */
async function flushMicrotasks() {
  await new Promise((resolve) => setTimeout(resolve, 0));
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
    FakeWebSocket.instances = [];
    vi.stubGlobal('WebSocket', FakeWebSocket);
    fetchSpy = vi.spyOn(globalThis, 'fetch');
    fetchSpy.mockClear();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('mine le ticket et ouvre wss://<wsHost><path>?ticket=... — résout à l\'event open', async () => {
    (fetchSpy as any).mockResolvedValue(
      new Response(
        JSON.stringify({ ticket: 'abc', wsHost: 'my-sandbox.sandboxes.example.com' }),
        { status: 200 },
      ),
    );

    const promise = openSandboxWs('my-sandbox', '/ws/fs');
    await flushMicrotasks();

    expect(fetchSpy).toHaveBeenCalledWith(
      '/api/sandboxes/my-sandbox/ws-ticket',
      expect.any(Object),
    );
    // Régression : la promesse ne doit PAS résoudre avant l'event 'open'
    // (sinon un appelant qui envoie immédiatement — ex. Explorer.vue sur
    // l'auto-expand de la racine — cible un socket encore CONNECTING).
    let resolved = false;
    promise.then(() => {
      resolved = true;
    });
    await flushMicrotasks();
    expect(resolved).toBe(false);

    FakeWebSocket.instances[0].emitOpen();
    const ws = await promise;

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

    const promise = openSandboxWs('my-sandbox', '/ws/terminal');
    await flushMicrotasks();
    FakeWebSocket.instances[0].emitOpen();
    const ws = await promise;

    expect((ws as any).url).toBe(
      'wss://my-sandbox.sandboxes.example.com/ws/terminal?ticket=xyz',
    );
  });

  it("rejette si le WebSocket émet 'error' avant 'open'", async () => {
    (fetchSpy as any).mockResolvedValue(
      new Response(
        JSON.stringify({ ticket: 'abc', wsHost: 'my-sandbox.sandboxes.example.com' }),
        { status: 200 },
      ),
    );

    const promise = openSandboxWs('my-sandbox', '/ws/fs');
    await flushMicrotasks();
    FakeWebSocket.instances[0].emitError();

    await expect(promise).rejects.toThrow();
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
