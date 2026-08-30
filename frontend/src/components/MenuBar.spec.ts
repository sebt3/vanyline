import { createMemoryHistory, createRouter } from 'vue-router';
import { mount } from '@vue/test-utils';
import { afterEach, describe, expect, it, vi } from 'vitest';
import MenuBar from './MenuBar.vue';
import { clearIdeActions, registerIdeActions, useIdeSession } from '../composables/useIdeSession';

/** Ouvre `menuLabel` (pointerdown, pas click — cf. reka-ui) puis clique
 *  l'item dont le texte contient `itemLabel`. Le contenu du menu est
 *  téléporté dans document.body (MenubarPortal). */
async function openMenuAndClick(wrapper: ReturnType<typeof mount>, menuLabel: string, itemLabel: string) {
  const trigger = wrapper.find(`[data-value="${menuLabel}"]`);
  expect(trigger.exists()).toBe(true);
  trigger.element.dispatchEvent(
    new PointerEvent('pointerdown', { button: 0, bubbles: true, cancelable: true }),
  );
  await wrapper.vm.$nextTick();
  await new Promise((r) => setTimeout(r, 0));

  const items = [...document.querySelectorAll('[role="menuitem"]')];
  const item = items.find((el) => el.textContent?.includes(itemLabel));
  expect(item).toBeTruthy();
  item!.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
  await wrapper.vm.$nextTick();
  await new Promise((r) => setTimeout(r, 0));
}

describe('MenuBar', () => {
  afterEach(() => {
    clearIdeActions();
  });

  it("Enregistrer appelle ideActions.saveActiveFile", async () => {
    const router = createRouter({
      history: createMemoryHistory(),
      routes: [{ path: '/start', component: { template: '<div>Start</div>' } }],
    });
    const wrapper = mount(MenuBar, { global: { plugins: [router] } });
    await router.push('/start');
    await router.isReady();

    const save = vi.fn();
    registerIdeActions({ saveActiveFile: save });

    await openMenuAndClick(wrapper, 'Fichier', 'Enregistrer');

    expect(save).toHaveBeenCalledTimes(1);
  });

  it("Fermer l'onglet appelle ideActions.closeActiveTab", async () => {
    const router = createRouter({
      history: createMemoryHistory(),
      routes: [{ path: '/start', component: { template: '<div>Start</div>' } }],
    });
    const wrapper = mount(MenuBar, { global: { plugins: [router] } });
    await router.push('/start');
    await router.isReady();

    const closeTab = vi.fn();
    registerIdeActions({ closeActiveTab: closeTab });

    await openMenuAndClick(wrapper, 'Fichier', "Fermer l'onglet");

    expect(closeTab).toHaveBeenCalledTimes(1);
  });

  it('Vers le projet navigue vers /p/:projectName depuis la route sandbox', async () => {
    const router = createRouter({
      history: createMemoryHistory(),
      routes: [
        { path: '/p/:projectName', component: { template: '<div>Project</div>' } },
        {
          path: '/p/:projectName/s/:sandboxName',
          component: { template: '<div>Sandbox</div>' },
        },
      ],
    });
    const wrapper = mount(MenuBar, { global: { plugins: [router] } });
    await router.push('/p/demo/s/main');
    await router.isReady();

    await openMenuAndClick(wrapper, 'Fichier', 'Vers le projet');

    expect(router.currentRoute.value.path).toBe('/p/demo');
  });

  it('Explorer (Affichage) appelle ideActions.openExplorer', async () => {
    const router = createRouter({
      history: createMemoryHistory(),
      routes: [{ path: '/start', component: { template: '<div>Start</div>' } }],
    });
    const wrapper = mount(MenuBar, { global: { plugins: [router] } });
    await router.push('/start');
    await router.isReady();

    const openExplorer = vi.fn();
    registerIdeActions({ openExplorer });

    await openMenuAndClick(wrapper, 'Affichage', 'Explorer');

    expect(openExplorer).toHaveBeenCalledTimes(1);
  });

  it('Nouveau terminal (Affichage) appelle ideActions.newTerminal', async () => {
    const router = createRouter({
      history: createMemoryHistory(),
      routes: [{ path: '/start', component: { template: '<div>Start</div>' } }],
    });
    const wrapper = mount(MenuBar, { global: { plugins: [router] } });
    await router.push('/start');
    await router.isReady();

    const newTerminal = vi.fn();
    registerIdeActions({ newTerminal });

    await openMenuAndClick(wrapper, 'Affichage', 'Nouveau terminal');

    expect(newTerminal).toHaveBeenCalledTimes(1);
  });

  it('Nouvelle session agent déclenche httpChatBackend.createConversation', async () => {
    const fetchSpy = vi.fn().mockImplementation((url: string) => {
      if (url === '/api/v1/agents') {
        return Promise.resolve(
          new Response(JSON.stringify({ items: [{ name: 'default' }], page: 1, per_page: 100, total_items: 1, total_pages: 1 }), {
            status: 200,
            headers: { 'content-type': 'application/json' },
          }),
        );
      }
      return Promise.resolve(
        new Response(JSON.stringify({ id: 42 }), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        }),
      );
    });
    vi.stubGlobal('fetch', fetchSpy);

    const router = createRouter({
      history: createMemoryHistory(),
      routes: [
        { path: '/p/:projectName/s/:sandboxName', component: { template: '<div>Sandbox</div>' } },
      ],
    });
    const wrapper = mount(MenuBar, { global: { plugins: [router] } });
    await router.push('/p/demo/s/main');
    await router.isReady();

    await openMenuAndClick(wrapper, 'Exécution', 'Nouvelle session agent');
    await new Promise((r) => setTimeout(r, 0));

    expect(useIdeSession().activeConversationId.value).toBe('42');
    const createCall = fetchSpy.mock.calls.find(([url]) => url === '/api/conversations');
    expect(JSON.parse((createCall?.[1] as RequestInit).body as string)).toEqual({
      agent_name: 'default',
      context: { kind: 'sandbox', data: { sandbox_name: 'main' } },
    });
    vi.unstubAllGlobals();
  });
  it('navigue vers /settings quand on clique sur Configuration', async () => {
    // Route initiale ≠ /settings : le test échoue si le clic ne déclenche pas
    // la navigation (pas de faux positif via un redirect initial).
    const router = createRouter({
      history: createMemoryHistory(),
      routes: [
        { path: '/start', component: { template: '<div>Start</div>' } },
        { path: '/settings', component: { template: '<div>Settings</div>' } },
      ],
    });

    const wrapper = mount(MenuBar, { global: { plugins: [router] } });
    await router.push('/start');
    await router.isReady();
    expect(router.currentRoute.value.path).toBe('/start');

    // 1. Ouvrir le menu "Affichage" : le trigger reka-ui ouvre le menu sur
    //    pointerdown (bouton gauche, sans ctrl) — pas un simple click.
    const trigger = wrapper.find('[data-value="Affichage"]');
    expect(trigger.exists()).toBe(true);
    trigger.element.dispatchEvent(
      new PointerEvent('pointerdown', { button: 0, bubbles: true, cancelable: true }),
    );
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    // 2. Le contenu du menu est téléporté dans document.body (MenubarPortal).
    //    L'item "Configuration" est rendu avec role="menuitem" : un click DOM
    //    déclenche le handler onClick de reka-ui qui émet @select.
    const items = [...document.querySelectorAll('[role="menuitem"]')];
    const configItem = items.find((el) => el.textContent?.includes('Configuration'));
    expect(configItem).toBeTruthy();
    configItem!.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
    await wrapper.vm.$nextTick();
    await new Promise((r) => setTimeout(r, 0));

    expect(router.currentRoute.value.path).toBe('/settings');
  });

  it('Rechercher (Édition) appelle ideActions.findInActiveFile', async () => {
    const router = createRouter({
      history: createMemoryHistory(),
      routes: [{ path: '/start', component: { template: '<div>Start</div>' } }],
    });
    const wrapper = mount(MenuBar, { global: { plugins: [router] } });
    await router.push('/start');
    await router.isReady();

    const findInActiveFile = vi.fn();
    registerIdeActions({ findInActiveFile });

    await openMenuAndClick(wrapper, 'Édition', 'Rechercher');

    expect(findInActiveFile).toHaveBeenCalledTimes(1);
  });

  it('Remplacer (Édition) appelle ideActions.replaceInActiveFile', async () => {
    const router = createRouter({
      history: createMemoryHistory(),
      routes: [{ path: '/start', component: { template: '<div>Start</div>' } }],
    });
    const wrapper = mount(MenuBar, { global: { plugins: [router] } });
    await router.push('/start');
    await router.isReady();

    const replaceInActiveFile = vi.fn();
    registerIdeActions({ replaceInActiveFile });

    await openMenuAndClick(wrapper, 'Édition', 'Remplacer');

    expect(replaceInActiveFile).toHaveBeenCalledTimes(1);
  });
});
