// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import ConfigView from './ConfigView.vue';
import { resetBridgeSingleton } from './bridge';

// Depuis la tâche 06a, ConfigView appelle getBridgeClient() dans son setup
// (pattern App.vue) : harness du pont global, comme dans app.spec.ts.
// acquireVsCodeApi doit exister AVANT le mount ; le singleton est recalé à
// chaque test pour repartir d'un client neuf.
let posted: Record<string, unknown>[] = [];

function emit(msg: unknown): void {
  window.dispatchEvent(new MessageEvent('message', { data: msg }));
}

/** Répond `result` à toutes les requêtes `config/*` déjà postées. */
function respondToConfigRequests(result: unknown): void {
  for (const msg of posted) {
    if (msg['type'] === 'rpc' && String(msg['method']).startsWith('config/')) {
      emit({ type: 'rpc/resp', reqId: msg['reqId'], ok: true, result });
    }
  }
}

beforeEach(() => {
  posted = [];
  window.acquireVsCodeApi = vi.fn(() => ({
    postMessage: (msg: unknown) => {
      posted.push(msg as Record<string, unknown>);
    },
  }));
  resetBridgeSingleton();
});

describe('ConfigView (panel config — repo RPC branché sur le pont)', () => {
  it('rend les 4 entrées de nav du port CLI (Compte absent)', async () => {
    const wrapper = mount(ConfigView);
    await flushPromises();

    const labels = wrapper.findAll('.nav-label').map((n) => n.text());
    expect(labels).toEqual(expect.arrayContaining(['Modèles', 'Outils', 'Agents', 'Skills']));
    // Pas de notion de compte côté CLI (F4) : le groupe account du frontend est exclu.
    expect(wrapper.text()).not.toContain('Compte');
    wrapper.unmount();
  });

  it('écran par défaut (llm-providers) — liste vide du pont factice → EmptyState', async () => {
    const wrapper = mount(ConfigView);
    await flushPromises();

    // Preuve que le repo branché remplace le stub : l'écran monté a interrogé
    // le pont (config/*) — le stub, lui, ne postait jamais rien.
    expect(posted.some((m) => m['type'] === 'rpc')).toBe(true);
    respondToConfigRequests([]);
    await flushPromises();

    // La nav rend toujours les 4 groupes…
    const labels = wrapper.findAll('.nav-label').map((n) => n.text());
    expect(labels).toEqual(expect.arrayContaining(['Modèles', 'Outils', 'Agents', 'Skills']));
    // …et l'écran par défaut rend l'EmptyState de liste vide : le stub
    // VNL-EXT-022 et son ErrorCard ont disparu.
    expect(wrapper.text()).toContain('Aucun fournisseur LLM.');
    expect(wrapper.find('[role="alert"]').exists()).toBe(false);
    wrapper.unmount();
  });
});
