import { afterEach, describe, expect, it, vi } from 'vitest';
import { defineComponent, h } from 'vue';
import { mount } from '@vue/test-utils';
import SettingsView from './SettingsView.vue';
import { activeNav } from './settings/navState';

describe('SettingsView', () => {
  it('affiche 5 groupes sans Projets ni Sandboxes', () => {
    const wrapper = mount(SettingsView);
    const items = wrapper.findAll('.nav-item');
    expect(items).toHaveLength(5);
    expect(wrapper.text()).toContain('Modèles');
    expect(wrapper.text()).toContain('Outils');
    expect(wrapper.text()).toContain('Agents');
    expect(wrapper.text()).toContain('Skills');
    expect(wrapper.text()).toContain('Compte');
    expect(wrapper.text()).not.toContain('Projets');
    expect(wrapper.text()).not.toContain('Sandboxes');
  });

  it('révèle 2 sous-items quand on clique sur "Modèles"', async () => {
    const wrapper = mount(SettingsView);
    const modelesItem = wrapper.find('[data-group="modeles"]');
    await modelesItem.trigger('click');
    await wrapper.vm.$nextTick();

    const subItems = wrapper.findAll('.nav-sub-item');
    expect(subItems).toHaveLength(2);
    expect(wrapper.text()).toContain('Fournisseurs LLM');
    expect(wrapper.text()).toContain('Profils de modèle');
  });

  it('révèle 2 sous-items quand on clique sur "Outils"', async () => {
    // D'abord cliquer sur un autre groupe pour reset
    const wrapper = mount(SettingsView);
    await wrapper.find('[data-group="agents"]').trigger('click');
    await wrapper.vm.$nextTick();

    // Maintenant cliquer Outils
    await wrapper.find('[data-group="outils"]').trigger('click');
    await wrapper.vm.$nextTick();

    const subItems = wrapper.findAll('.nav-sub-item');
    expect(subItems).toHaveLength(2);
    expect(wrapper.text()).toContain('Serveurs MCPs');
    expect(wrapper.text()).toContain('Toolsets');
  });

  it('monte les écrans Agents, Skills et Account', async () => {
    const wrapper = mount(SettingsView);

    const navItems = wrapper.findAll('.nav-item');

    // Agents → AgentsScreen
    await navItems[2].trigger('click');
    await wrapper.vm.$nextTick();
    expect(wrapper.findComponent({ name: 'AgentsScreen' }).exists()).toBe(true);

    // Skills → SkillsScreen
    await navItems[3].trigger('click');
    await wrapper.vm.$nextTick();
    expect(wrapper.findComponent({ name: 'SkillsScreen' }).exists()).toBe(true);

    // Compte → AccountScreen
    await navItems[4].trigger('click');
    await wrapper.vm.$nextTick();
    expect(wrapper.findComponent({ name: 'AccountScreen' }).exists()).toBe(true);
  });

  it('synchronise activeNav après clic sur Outils', async () => {
    const wrapper = mount(SettingsView);

    // Cliquer Outils
    await wrapper.find('[data-group="outils"]').trigger('click');
    await wrapper.vm.$nextTick();

    expect(activeNav.value).toEqual({
      groupLabel: 'Outils',
      screenLabel: 'Serveurs MCPs',
    });
  });
});

// ---------------------------------------------------------------------------
// SettingsView.sdd — assistant de configuration (ContextChat stubé, émet
// `turn-finished`), premier démarrage sans profil, mise en page 360 px.
// Le groupe « Global » (administrateurs) n'est PAS livré : l'API `/api/me`
// ne porte pas les groupes (cf. rapport G2a).
// ---------------------------------------------------------------------------

const ContextChatStub = defineComponent({
  name: 'ContextChat',
  props: { kind: String, data: Object },
  emits: ['turn-finished'],
  setup() {
    return () => h('div', { class: 'ctx-stub' });
  },
});

function setWidth(width: number) {
  Object.defineProperty(window, 'innerWidth', { configurable: true, writable: true, value: width });
}

async function flush() {
  for (let i = 0; i < 6; i++) await new Promise((r) => setTimeout(r, 0));
}

function jsonRes(body: unknown) {
  return {
    ok: true,
    status: 200,
    headers: new Map([['content-type', 'application/json']]),
    json: () => Promise.resolve(body),
  } as unknown as Response;
}

function configFetch(profiles: unknown[]) {
  const spy = vi.spyOn(globalThis, 'fetch');
  spy.mockImplementation(async (url: string | URL | Request) => {
    const u = String(url);
    if (u.startsWith('/api/v1/model-profiles')) return jsonRes(profiles);
    return jsonRes([]);
  });
  return spy;
}

function countCalls(spy: { mock: { calls: unknown[][] } }, prefix: string) {
  return spy.mock.calls.filter((call) => String(call[0]).startsWith(prefix)).length;
}

describe('SettingsView — SettingsView.sdd (G2a)', () => {
  afterEach(() => {
    setWidth(1024);
    vi.restoreAllMocks();
  });

  it('configuration_par_l_assistant', async () => {
    setWidth(1280);
    const spy = configFetch([{ id: 1, name: 'profil' }]);
    const wrapper = mount(SettingsView, { global: { stubs: { ContextChat: ContextChatStub } } });
    await flush();
    await wrapper.find('[data-group="outils"]').trigger('click');
    await flush();
    const before = countCalls(spy, '/api/v1/mcp-servers');
    expect(wrapper.findComponent({ name: 'ContextChat' }).exists()).toBe(true);
    wrapper.findComponent({ name: 'ContextChat' }).vm.$emit('turn-finished');
    await flush();
    expect(countCalls(spy, '/api/v1/mcp-servers')).toBeGreaterThan(before);
  });

  it('premier_demarrage_sans_profil', async () => {
    setWidth(1280);
    configFetch([]);
    const wrapper = mount(SettingsView, { global: { stubs: { ContextChat: ContextChatStub } } });
    await flush();
    expect(wrapper.findComponent({ name: 'ContextChat' }).exists()).toBe(false);
    expect(wrapper.text()).toContain('Aucun profil de modèle');
    expect(activeNav.value.screenLabel).toBe('Fournisseurs LLM');
  });

  it('mise_en_page_mobile', async () => {
    setWidth(360);
    configFetch([{ id: 1, name: 'profil' }]);
    const wrapper = mount(SettingsView, { global: { stubs: { ContextChat: ContextChatStub } } });
    await flush();
    expect(wrapper.find('[data-layout="tab"]').exists()).toBe(false);
    const tab = wrapper.findAll('[role="tab"]').find((b) => b.text() === 'Assistant');
    expect(tab).toBeDefined();
    await tab!.trigger('click');
    await flush();
    expect(wrapper.find('[data-layout="tab"]').exists()).toBe(true);
    expect(wrapper.findComponent({ name: 'ContextChat' }).exists()).toBe(true);
  });
});

describe('SettingsView — Global (G2a, SettingsView.sdd)', () => {
  afterEach(() => {
    setWidth(1024);
    vi.restoreAllMocks();
  });

  it('reglages_globaux_pour_les_admins', async () => {
    setWidth(1280);
    const spy = vi.spyOn(globalThis, 'fetch');
    spy.mockImplementation(async (url: string | URL | Request) => {
      const u = String(url);
      if (u === '/api/v1/me') return jsonRes({ subject: 'a', email: null, groups: ['admin'] });
      if (u.startsWith('/api/v1/model-profiles')) return jsonRes([{ id: 1, name: 'p' }]);
      return jsonRes([]);
    });
    const admin = mount(SettingsView, { global: { stubs: { ContextChat: ContextChatStub } } });
    await flush();
    expect(admin.findAll('.nav-item')).toHaveLength(6);
    expect(admin.text()).toContain('Global');
    admin.unmount();

    spy.mockImplementation(async (url: string | URL | Request) => {
      const u = String(url);
      if (u === '/api/v1/me') return jsonRes({ subject: 'u', email: null, groups: [] });
      if (u.startsWith('/api/v1/model-profiles')) return jsonRes([{ id: 1, name: 'p' }]);
      return jsonRes([]);
    });
    const ordinary = mount(SettingsView, { global: { stubs: { ContextChat: ContextChatStub } } });
    await flush();
    expect(ordinary.findAll('.nav-item')).toHaveLength(5);
    expect(ordinary.text()).not.toContain('Global');
  });
});
