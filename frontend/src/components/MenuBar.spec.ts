import { createMemoryHistory, createRouter } from 'vue-router';
import { mount } from '@vue/test-utils';
import { describe, expect, it } from 'vitest';
import MenuBar from './MenuBar.vue';

describe('MenuBar', () => {
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
});
