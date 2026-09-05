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

  it('envoie {op, id, ...params} et résout la réponse ok', async () => {
    ws = new FakeWebSocket('wss://example.com/ws/fs');
    const client = new SandboxFsClient(ws as unknown as WebSocket);

    const p = client.request('list', { path: '.' });

    // Flushed la microtask programmée par queue.then(run, run)
    await flushQueue();

    // Tâche 08b : la requête porte un id de corrélation auto-incrémenté.
    expect(ws.sent[0]).toBe('{"op":"list","id":1,"path":"."}');
    ws.emitMessage('{"ok":true,"entries":"a.txt","id":1}');

    const result = await p;
    expect(result).toEqual({ ok: true, entries: 'a.txt', id: 1 });
  });

  it('rejette quand ok est false', async () => {
    ws = new FakeWebSocket('wss://example.com/ws/fs');
    const client = new SandboxFsClient(ws as unknown as WebSocket);

    const p = client.request('read', { path: 'missing.txt' });

    await flushQueue();
    ws.emitMessage('{"ok":false,"error":"boom","id":1}');

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
    ws.emitMessage('{"ok":true,"content":"aaa","id":1}');
    await flushQueue();
    expect(ws.sent.length).toBe(2);
    expect(ws.sent[1]).toBe('{"op":"read","id":2,"path":"b.txt"}');

    const r1 = await p1;
    expect(r1).toEqual({ ok: true, content: 'aaa', id: 1 });
    p2.catch(() => null); // sérialisation vérifiée : p2 programmé dans la queue
  });

  it('résout par id : une réponse d’un id inconnu est ignorée, la requête en vol attend la sienne', async () => {
    ws = new FakeWebSocket('wss://example.com/ws/fs');
    const client = new SandboxFsClient(ws as unknown as WebSocket);

    const p = client.request('read', { path: 'a.txt' });
    await flushQueue();
    expect(JSON.parse(ws.sent[0]).id).toBe(1);

    // id 99 inconnu : frame ignorée, la requête en vol n'est pas résolue.
    ws.emitMessage('{"ok":true,"content":"mauvais","id":99}');
    let settled = false;
    p.then(() => {
      settled = true;
    });
    await flushQueue();
    await flushQueue();
    expect(settled).toBe(false);

    // La bonne réponse (id 1) résout enfin.
    ws.emitMessage('{"ok":true,"content":"c","id":1}');
    const result = await p;
    expect(result).toEqual({ ok: true, content: 'c', id: 1 });
  });

  it('routage des événements : un événement ne consomme jamais la requête en vol', async () => {
    ws = new FakeWebSocket('wss://example.com/ws/fs');
    const client = new SandboxFsClient(ws as unknown as WebSocket);

    const handler = vi.fn();
    client.onEvent('file-changed', handler);

    const p = client.request('read', { path: 'a.txt' });
    await flushQueue();

    // Événement (sans id) pendant une requête en vol : notifie l'abonné, ne
    // résout PAS la requête (c'était le poison du listener one-shot : une
    // frame non sollicitée résolvait la requête avec le mauvais payload).
    ws.emitMessage('{"event":"file-changed","path":"a.rs"}');
    expect(handler).toHaveBeenCalledWith({ event: 'file-changed', path: 'a.rs' });
    let settled = false;
    p.then(() => {
      settled = true;
    });
    await flushQueue();
    await flushQueue();
    expect(settled).toBe(false);

    ws.emitMessage('{"ok":true,"content":"c","id":1}');
    expect(await p).toEqual({ ok: true, content: 'c', id: 1 });
  });

  it('onEvent : désabonnement rendu, plusieurs abonnés, handler qui throw n’empêche pas les autres', () => {
    ws = new FakeWebSocket('wss://example.com/ws/fs');
    const client = new SandboxFsClient(ws as unknown as WebSocket);

    const h1 = vi.fn(() => {
      throw new Error('abonné malade');
    });
    const h2 = vi.fn();
    const unsubscribe = client.onEvent('file-changed', h1);
    client.onEvent('file-changed', h2);

    ws.emitMessage('{"event":"file-changed","path":"a.rs"}');
    expect(h1).toHaveBeenCalledTimes(1);
    // Le throw de h1 est contenu (try/catch interne) : h2 tourne quand même.
    expect(h2).toHaveBeenCalledTimes(1);

    unsubscribe();
    ws.emitMessage('{"event":"file-changed","path":"b.rs"}');
    expect(h1).toHaveBeenCalledTimes(1);
    expect(h2).toHaveBeenCalledTimes(2);
  });

  it('fallback legacy : frame sans id ni event avec une requête en vol → la résout', async () => {
    // Serveur antérieur à 08b : réponses sans champ de corrélation — la
    // sémantique un-à-la-fois de la queue suffit à router la réponse.
    ws = new FakeWebSocket('wss://example.com/ws/fs');
    const client = new SandboxFsClient(ws as unknown as WebSocket);

    const p = client.request('read', { path: 'a.txt' });
    await flushQueue();

    ws.emitMessage('{"ok":true,"content":"legacy"}');
    expect(await p).toEqual({ ok: true, content: 'legacy' });
  });

  it('frames non JSON et sans intérêt (aucun abonné, aucune requête) : ignorées', async () => {
    ws = new FakeWebSocket('wss://example.com/ws/fs');
    // La seule construction du client installe le listener permanent — les
    // frames ci-dessous ne doivent lever aucune exception dans le dispatch.
    new SandboxFsClient(ws as unknown as WebSocket);

    // Non-JSON : pas de throw dans le dispatch permanent.
    ws.emitMessage('not json');
    // Event sans abonné : ignoré silencieusement.
    ws.emitMessage('{"event":"autre-chose","x":1}');
    // Sans id, sans event, sans requête en vol : ignoré.
    ws.emitMessage('{"ok":true,"content":"orpheline"}');
  });

  it('la queue survit à un aller-retour flush-ack (08c)', async () => {
    // Piège de l'aller-retour « flush avant écriture » : `request` pose son
    // propre id de corrélation PUIS spread les params — un params `{id}`
    // l'écraserait, le pending ne serait jamais résolu et la queue FIFO
    // mourrait. L'ack voyage donc dans `ackFor` ; assertion anti-blocage :
    // une requête SUIVANTE part bien après la réponse.
    ws = new FakeWebSocket('wss://example.com/ws/fs');
    const client = new SandboxFsClient(ws as unknown as WebSocket);

    const p = client.request('flush-ack', { ackFor: 7 });
    await flushQueue();
    expect(ws.sent[0]).toBe('{"op":"flush-ack","id":1,"ackFor":7}');

    // Réponse serveur : {ok:true} nu + id de corrélation attaché par la
    // boucle (attach_req_id, 08b) — le pending se solde normalement.
    ws.emitMessage('{"ok":true,"id":1}');
    expect(await p).toEqual({ ok: true, id: 1 });

    // Queue vivante : la requête suivante est envoyée avec son id 2.
    const p2 = client.request('read', { path: 'a.txt' });
    await flushQueue();
    expect(ws.sent.length).toBe(2);
    expect(ws.sent[1]).toBe('{"op":"read","id":2,"path":"a.txt"}');
    ws.emitMessage('{"ok":true,"content":"c","id":2}');
    expect(await p2).toEqual({ ok: true, content: 'c', id: 2 });
  });
});
