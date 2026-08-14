import { beforeEach, describe, expect, it, vi } from 'vitest';
import { chatWsUrl, openChatWs } from './chatWs';

class FakeWebSocket {
  static instances: FakeWebSocket[] = [];
  url: string;
  private listeners: Record<string, Array<(ev: { type?: string }) => void>> = {};
  constructor(url: string) {
    this.url = url;
    FakeWebSocket.instances.push(this);
  }
  addEventListener(type: string, cb: (ev: { type?: string }) => void) {
    (this.listeners[type] ??= []).push(cb);
  }
  removeEventListener() {
    /* no-op, suffisant pour ces tests */
  }
  emitOpen() {
    for (const cb of [...(this.listeners['open'] ?? [])]) cb({ type: 'open' });
  }
  emitError() {
    for (const cb of [...(this.listeners['error'] ?? [])]) cb({ type: 'error' });
  }
}

describe('chatWsUrl', () => {
  it('construit une URL same-origin ws:// en http', () => {
    expect(chatWsUrl('abc')).toBe(`ws://${location.host}/api/ws/chat/abc`);
  });
});

describe('openChatWs', () => {
  beforeEach(() => {
    FakeWebSocket.instances = [];
    vi.stubGlobal('WebSocket', FakeWebSocket);
  });

  it("ne résout qu'à l'event open", async () => {
    const promise = openChatWs('conv-1');
    let resolved = false;
    promise.then(() => {
      resolved = true;
    });

    await Promise.resolve();
    expect(resolved).toBe(false);

    FakeWebSocket.instances[0].emitOpen();
    const ws = await promise;
    expect((ws as unknown as FakeWebSocket).url).toContain('/api/ws/chat/conv-1');
  });

  it("rejette sur 'error'", async () => {
    const promise = openChatWs('conv-1');
    await Promise.resolve();
    FakeWebSocket.instances[0].emitError();

    await expect(promise).rejects.toThrow();
  });
});
