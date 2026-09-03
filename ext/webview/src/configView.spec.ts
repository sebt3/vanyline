// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import ConfigView from './ConfigView.vue';

// Le stub configRepo n'appelle jamais acquireVsCodeApi (contrairement au pont
// chat d'App.vue) — aucun harness de pont nécessaire ici.
describe('ConfigView (panel config — hello ConfigShell)', () => {
  it('rend les 4 entrées de nav du port CLI (Compte absent)', async () => {
    const wrapper = mount(ConfigView);
    await flushPromises();

    const labels = wrapper.findAll('.nav-label').map((n) => n.text());
    expect(labels).toEqual(expect.arrayContaining(['Modèles', 'Outils', 'Agents', 'Skills']));
    // Pas de notion de compte côté CLI (F4) : le groupe account du frontend est exclu.
    expect(wrapper.text()).not.toContain('Compte');
    wrapper.unmount();
  });

  it('écran par défaut (llm-providers) monté → ErrorCard du stub (VNL-EXT-022)', async () => {
    const wrapper = mount(ConfigView);
    await flushPromises();

    // Preuve que le repo fourni est injecté et que le shell rend : la liste en
    // ErrorCard porte le code d'erreur du stub (repo non branché, tâche 06).
    const card = wrapper.find('[role="alert"]');
    expect(card.exists()).toBe(true);
    expect(card.text()).toContain('VNL-EXT-022');
    wrapper.unmount();
  });
});
