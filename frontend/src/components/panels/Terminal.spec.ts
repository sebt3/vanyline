/* eslint-disable @typescript-eslint/no-explicit-any */
import { mount } from '@vue/test-utils';
import { afterEach, describe, expect, it, vi } from 'vitest';
import Terminal from './Terminal.vue';
import { openSandboxWs } from '../../api/sandboxWs';

// vi.hoisted : exécuté avant vi.mock — mêmes références entre module et test.
const { terminalInstances, wsInstances, resizeInstances } = vi.hoisted(() => {
  const terminalInstances: Array<{
    onDataCb?: (d: string) => void;
    written: unknown[];
    cols: number;
    rows: number;
    options: Record<string, unknown>;
  }> = [];
  const wsInstances: Array<{
    url: string;
    binaryType: string;
    readyState: number;
    sent: unknown[];
    listeners: Record<string, any[]>;
    addEventListener(t: string, cb: (ev: any) => void): void;
    emitMessage(ev: any): void;
    close(): void;
  }> = [];
  const resizeInstances: Array<{ cb: () => void }> = [];

  return {
    terminalInstances,
    wsInstances,
    resizeInstances,
  };
});

vi.mock('@xterm/xterm', () => ({
  Terminal: class {
    cols = 80;
    rows = 24;
    onDataCb?: (d: string) => void;
    written: unknown[] = [];
    options: Record<string, unknown>;

    constructor(opts: Record<string, unknown>) {
      this.options = opts;
      (terminalInstances as any).push(this as any);
    }

    loadAddon() {}
    open() {}

    onData(cb: (d: string) => void) {
      this.onDataCb = cb;
    }

    write(data: unknown) {
      this.written.push(data);
    }

    dispose() {}
  },
}));

vi.mock('@xterm/addon-fit', () => ({
  FitAddon: class {
    fit() {}
  },
}));

vi.mock('../../api/sandboxWs', () => ({
  openSandboxWs: vi.fn(),
}));

// Stubs globaux : ResizeObserver + WebSocket (OPEN = 1).
const g = globalThis as { ResizeObserver?: unknown; ResizeObserverStub?: unknown };
g.ResizeObserver = g.ResizeObserverStub =
  class ResizeObserverStub {
    cb: () => void;
    // eslint-disable-next-line @typescript-eslint/no-unused-vars
    constructor(cb: () => void) {
      this.cb = cb;
      (resizeInstances as any[]).push({ cb });
    }
    observe() {}
    disconnect() {}
  };

class FakeWebSocket {
  static CONNECTING = 0;
  static OPEN = 1;
  listeners: Record<string, any[]> = {};
  url: string;
  binaryType = '';
  // CONNECTING au départ, comme un vrai WebSocket — la promesse d'ouverture
  // se résout à la construction, pas à l'ouverture réelle (cf. Terminal.vue).
  readyState = 0;
  sent: unknown[] = [];

  constructor(url: string) {
    this.url = url;
    (wsInstances as any[]).push(this as any);
  }

  addEventListener(type: string, cb: (ev: any) => void) {
    (this.listeners[type] ??= []).push(cb);
  }

  removeEventListener(_type: string, _cb: (ev: any) => void) {
    /* no-op */
  }

  send(data: unknown) {
    this.sent.push(data);
  }

  emitMessage(data: unknown) {
    for (const cb of [...(this.listeners['message'] ?? [])]) {
      cb({ data });
    }
  }

  /** Simule l'event 'open' réel — readyState passe à OPEN et les listeners
   *  { once: true } enregistrés (ex. sendResize dans Terminal.vue) se déclenchent. */
  emitOpen() {
    this.readyState = FakeWebSocket.OPEN;
    for (const cb of [...(this.listeners['open'] ?? [])]) {
      cb({});
    }
  }

  close() {}
}

vi.stubGlobal('WebSocket', FakeWebSocket);

function flushTwo() {
  return Promise.resolve().then(() => Promise.resolve());
}

describe('Terminal.vue — PTY réel', () => {
  afterEach(() => {
    (openSandboxWs as any).mockClear();
    (terminalInstances as any[]).length = 0;
    (wsInstances as any[]).length = 0;
    (resizeInstances as any[]).length = 0;
  });

  it('ouvre une connexion /ws/terminal dédiée', async () => {
    const ws = new FakeWebSocket('wss://example.com/ws/terminal?ticket=abc');
    (openSandboxWs as any).mockImplementation(() => Promise.resolve(ws));

    mount(Terminal, {
      global: { provide: { 'sandbox-name': 'foo' } },
    });

    await flushTwo();

    expect(openSandboxWs).toHaveBeenCalledWith('foo', '/ws/terminal');
    expect((wsInstances as any[]).length).toBe(1);
    const w = (wsInstances as any[])[0];
    expect(w.binaryType).toBe('arraybuffer');
    expect(w.listeners['message']).toHaveLength(1);
  });

  it("n'envoie pas de resize avant l'event 'open' du WebSocket", async () => {
    // Régression : la promesse d'ouverture se résout à la construction du
    // WebSocket (CONNECTING), pas à l'ouverture réelle. Un sendResize()
    // envoyé à ce moment-là serait un no-op silencieux dans un vrai
    // navigateur (readyState !== OPEN) — ce test verrouille qu'on attend
    // bien l'event 'open' plutôt que d'appeler sendResize() trop tôt.
    const ws = new FakeWebSocket('wss://example.com/ws/terminal?ticket=abc');
    (openSandboxWs as any).mockImplementation(() => Promise.resolve(ws));

    mount(Terminal, {
      global: { provide: { 'sandbox-name': 'foo' } },
    });

    await flushTwo();

    expect(ws.readyState).toBe(FakeWebSocket.CONNECTING);
    expect(ws.sent).toHaveLength(0);

    ws.emitOpen();

    expect(ws.sent).toHaveLength(1);
    expect(ws.sent[0]).toBe('{"type":"resize","cols":80,"rows":24}');
  });

  it('entrée utilisateur → frame binaire', async () => {
    const ws = new FakeWebSocket('wss://example.com/ws/terminal?ticket=abc');
    (openSandboxWs as any).mockImplementation(() => Promise.resolve(ws));

    mount(Terminal, {
      global: { provide: { 'sandbox-name': 'foo' } },
    });

    await flushTwo();
    ws.emitOpen(); // déclenche le sendResize initial (listener 'open')

    const term = (terminalInstances as any[])[0];
    expect(term.onDataCb).toBeDefined();

    // Simuler un input utilisateur
    term.onDataCb!('x');
    // ws.sent contient d'abord le resize initial, puis la frame data
    expect(ws.sent.length).toBeGreaterThanOrEqual(2);
    // La dernière frame est celle de l'entrée utilisateur
    const expectedBytes = new TextEncoder().encode('x');
    const last = ws.sent[ws.sent.length - 1] as unknown;
    expect(last).toEqual(expectedBytes);
  });

  it('frame binaire entrante → term.write(Uint8Array)', async () => {
    const ws = new FakeWebSocket('wss://example.com/ws/terminal?ticket=abc');
    (openSandboxWs as any).mockImplementation(() => Promise.resolve(ws));

    mount(Terminal, {
      global: { provide: { 'sandbox-name': 'foo' } },
    });

    await flushTwo();

    const term = (terminalInstances as any[])[0];
    expect(term.onDataCb).toBeDefined();

    // Déclenche le handler message sur le ws
    ws.emitMessage(new Uint8Array([104, 105]));

    const written = (term as any)?.written?.[0];
    const expected = new Uint8Array([104, 105]);
    expect(written).toEqual(expected);
  });

  it('resize → frame texte {"type":"resize",cols,rows}', async () => {
    const ws = new FakeWebSocket('wss://example.com/ws/terminal?ticket=abc');
    (openSandboxWs as any).mockImplementation(() => Promise.resolve(ws));

    mount(Terminal, {
      global: { provide: { 'sandbox-name': 'foo' } },
    });

    await flushTwo();
    ws.emitOpen(); // déclenche le sendResize initial (listener 'open')

    const term = (terminalInstances as any[])[0];
    expect(term.onDataCb).toBeDefined();

    (term as any).cols = 100;
    (term as any).rows = 40;
    resizeInstances[0].cb();

    // après resize, ws.sent contient au moins 2 frames
    expect(ws.sent.length).toBeGreaterThanOrEqual(2);
    const msg = ws.sent[ws.sent.length - 1];
    expect(msg).toBe('{"type":"resize","cols":100,"rows":40}');
  });

  it('échec du ticket → terminal vide sans crash', async () => {
    (openSandboxWs as any).mockRejectedValueOnce(new Error('ticket'));

    mount(Terminal, {
      global: { provide: { 'sandbox-name': 'foo' } },
    });

    await flushTwo();

    // Le ws du composant n'a pas été connecté (rejet du ticket).
    expect((wsInstances as any[]).length).toBe(0);
  });
});
