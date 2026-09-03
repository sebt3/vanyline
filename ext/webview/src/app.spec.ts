// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import App from './App.vue';
import { resetBridgeSingleton } from './bridge';

// jsdom n'implémente pas scrollIntoView — Nuxt UI ChatMessages l'appelle au mount
// de ChatSession (même correctif que packages/ui/src/test-setup.ts, local au harness).
if (!Element.prototype.scrollIntoView) {
  Element.prototype.scrollIntoView = () => {};
}

let posted: Record<string, unknown>[] = [];

function lastRpc(method: string): Record<string, unknown> | undefined {
  return posted.filter((m) => m['type'] === 'rpc' && m['method'] === method).at(-1);
}

function emit(msg: unknown): void {
  window.dispatchEvent(new MessageEvent('message', { data: msg }));
}

beforeEach(() => {
  posted = [];
  // acquireVsCodeApi doit exister AVANT le mount (getBridgeClient l'appelle dans
  // le setup d'App.vue) ; le singleton est recalé pour repartir d'un client neuf.
  window.acquireVsCodeApi = vi.fn(() => ({
    postMessage: (msg: unknown) => {
      posted.push(msg as Record<string, unknown>);
    },
  }));
  resetBridgeSingleton();
});

describe('App (webview 04b — ports réels sur le pont)', () => {
  it('cas 15 — monte ChatWindow (.chat-host, test spike 01 conservé) + sélecteur d\'agent peuplé par config/agents', async () => {
    const wrapper = mount(App);
    await flushPromises();

    const req = lastRpc('config/agents');
    expect(req).toBeDefined();
    emit({
      type: 'rpc/resp',
      reqId: req?.['reqId'],
      ok: true,
      result: [{ name: 'orchestrator' }],
    });
    await flushPromises();

    const select = wrapper.find('select[data-testid=agent-select]');
    expect(select.exists()).toBe(true);
    expect(select.find('option[value=""]').text()).toContain('Agent (par défaut)');
    expect(select.findAll('option').map((o) => o.text())).toContain('orchestrator');
    // Agents reçus → le select n'est pas en UI dégradée.
    expect(select.attributes('disabled')).toBeUndefined();

    // Le test spike 01 survit : ChatWindow monté.
    expect(wrapper.find('.chat-host').exists()).toBe(true);
    wrapper.unmount();
  });

  it('cas 15 bis — config/agents en échec (VNL-EXT-021) → select désactivé, ChatWindow toujours monté', async () => {
    const wrapper = mount(App);
    await flushPromises();

    const req = lastRpc('config/agents');
    expect(req).toBeDefined();
    emit({
      type: 'rpc/resp',
      reqId: req?.['reqId'],
      ok: false,
      error: { code: 'VNL-EXT-021', message: 'serveur vanyline non démarré' },
    });
    await flushPromises();

    expect(wrapper.find('select[data-testid=agent-select]').attributes('disabled')).toBeDefined();
    expect(wrapper.find('.chat-host').exists()).toBe(true);
    wrapper.unmount();
  });

  it('cas 16 — session/pick du host → activeConversationId propagé : ChatSession monté appelle loadMessages(abc)', async () => {
    const wrapper = mount(App);
    await flushPromises();

    emit({ type: 'session/pick', conversationId: 'abc' });
    await flushPromises();

    // Pas de crash : ChatWindow en place, ChatSession monté sur la session reprise…
    expect(wrapper.find('.chat-host').exists()).toBe(true);
    expect(wrapper.find('.chat-session').exists()).toBe(true);

    // …et l'appel chatBackend.loadMessages('abc') est relayé en RPC conversations/get
    // sur le pont (preuve explicite de la propagation de activeConversationId).
    const get = lastRpc('conversations/get');
    expect(get).toBeDefined();
    expect(get?.['params']).toEqual({ id: 'abc' });
    wrapper.unmount();
  });
});
