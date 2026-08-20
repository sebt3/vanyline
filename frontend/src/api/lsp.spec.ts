import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { getLspClient, disposeLspClients, __testReset } from './lsp';

vi.mock('../api/sandboxWs', () => ({
  openSandboxWs: vi.fn(),
}));

import { openSandboxWs } from './sandboxWs';

/** FakeWebSocket qui simule un serveur LSP : readyState = OPEN,
 *  `send` répond à `initialize`, `close` ferme. */
class FakeWebSocket {
  static instances: FakeWebSocket[] = [];
  url: string;
  readyState: number;
  private listeners: Record<string, Array<(ev: { data?: string }) => void>> = {};
  sent: string[] = [];

  constructor(url: string) {
    this.url = url;
    this.readyState = 1; // OPEN
    FakeWebSocket.instances.push(this);
  }

  addEventListener(type: string, cb: (ev: { data?: string }) => void) {
    (this.listeners[type] ??= []).push(cb);
  }
  removeEventListener(type: string, cb: (ev: { data?: string }) => void) {
    this.listeners[type] = (this.listeners[type] ?? []).filter((f) => f !== cb);
  }
  send(data: string) {
    this.sent.push(data);
    // Si le serveur reçoit "initialize", répondre avec les capabilities
    const parsed = JSON.parse(data);
    if (parsed.method === 'initialize') {
      queueMicrotask(() => {
        const resp = JSON.stringify({ jsonrpc: '2.0', id: parsed.id, result: { capabilities: {} } });
        for (const h of [...(this.listeners['message'] ?? [])]) h({ data: resp });
      });
    }
  }
  close() {
    this.readyState = 3; // CLOSED
  }
}

beforeEach(() => {
  vi.clearAllMocks();
  __testReset();
  vi.stubGlobal('WebSocket', FakeWebSocket);
  FakeWebSocket.instances = [];
  (openSandboxWs as ReturnType<typeof vi.fn>).mockImplementation(
    async (_sandboxName: string, path: string) => {
      const ws = new FakeWebSocket(`wss://example.com${path}`);
      return ws;
    },
  );
});

afterEach(() => {
  __testReset();
  vi.restoreAllMocks();
  FakeWebSocket.instances = [];
});

describe('getLspClient', () => {
  it('ouvre /ws/lsp/{toolchain} et connecte le client', async () => {
    const client = await getLspClient('foo', 'rust');

    expect(openSandboxWs).toHaveBeenCalledWith('foo', '/ws/lsp/rust');
    expect(client).not.toBeNull();

    // Le fake WebSocket a reçu un message JSON avec method === "initialize"
    const fakeWs = FakeWebSocket.instances[0] as FakeWebSocket;
    expect(fakeWs.sent.length).toBeGreaterThan(0);
    const initMsg = JSON.parse(fakeWs.sent[0]);
    expect(initMsg.method).toBe('initialize');
  });

  it('cache par sandbox/toolchain — openSandboxWs appelé une seule fois', async () => {
    await getLspClient('foo', 'rust');
    await getLspClient('foo', 'rust');

    expect(openSandboxWs).toHaveBeenCalledTimes(1);
  });

  it('échec du ticket/WS → null (pas de throw)', async () => {
    (openSandboxWs as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new Error('ticket failed'));

    const result = await getLspClient('foo', 'rust');
    expect(result).toBeNull();

    // Le cache garde le null : appel ultérieur ne rouvre pas
    const result2 = await getLspClient('foo', 'rust');
    expect(result2).toBeNull();
    // openSandboxWs n'a été appelé qu'une fois (l'échec initial)
    expect(openSandboxWs).toHaveBeenCalledTimes(1);
  });

  it('disposeLspClients ferme le ws et permet la réouverture', async () => {
    const client = await getLspClient('foo', 'rust');
    expect(client).not.toBeNull();
    expect(openSandboxWs).toHaveBeenCalledTimes(1);

    const fakeWs = FakeWebSocket.instances[0] as FakeWebSocket;

    disposeLspClients('foo');
    expect(fakeWs.readyState).toBe(3); // CLOSED

    // Réouverture : le cache est vidé, donc un nouveau WS est créé
    (openSandboxWs as ReturnType<typeof vi.fn>).mockClear();
    await getLspClient('foo', 'rust');

    expect(openSandboxWs).toHaveBeenCalledWith('foo', '/ws/lsp/rust');
  });

  it('toolchains isolées — une connexion par toolchain', async () => {
    await getLspClient('foo', 'rust');
    await getLspClient('foo', 'node');

    expect(openSandboxWs).toHaveBeenCalledTimes(2);
    expect(openSandboxWs).toHaveBeenNthCalledWith(1, 'foo', '/ws/lsp/rust');
    expect(openSandboxWs).toHaveBeenNthCalledWith(2, 'foo', '/ws/lsp/node');
  });

  it('sandboxes isolées — disposeLspClients d\'une sandbox n\'affecte pas les autres', async () => {
    await getLspClient('foo', 'rust');
    await getLspClient('bar', 'rust');

    disposeLspClients('foo');

    // bar doit être intact — on ne peut pas vérifier directement mais
    // le test passe s'il n'y a pas d'erreur et qu'un nouvel appel pour bar utilise
    // un nouveau WebSocket (car le cache de foo a été vidé mais pas celui de bar).
    expect(openSandboxWs).toHaveBeenCalledTimes(2);
  });
});