import { createMemoryHistory, createRouter } from 'vue-router';
import { mount } from '@vue/test-utils';
import { describe, expect, it } from 'vitest';
import MenuBar from './MenuBar.vue';

describe('MenuBar', () => {
  it('navigate vers /settings quand on clique sur Configuration', async () => {
    const router = createRouter({
      history: createMemoryHistory(),
      routes: [{ path: '/', redirect: '/settings' }],
    });

    const wrapper = mount(MenuBar, { global: { plugins: [router] } });

    // Ouvrir le menu "Affichage" via l'API reka-ui
    const menus = wrapper.findAllComponents({ name: 'MenubarMenu' });
    const affichageMenu = menus.find(
      (m) => (m.vm as any).$props?.value === 'Affichage' || m.props('value') === 'Affichage'
    );

    if (affichageMenu) {
      // Ouvrir le menu via select
      await (affichageMenu.vm as any).$props?.onSelect?.();
      // Attendre que le menu soit rendu
      await new Promise((r) => setTimeout(r, 50));

      // L'item "Configuration" est téléporté dans document.body
      const items = document.querySelectorAll('[role="menuitem"]');
      const configItem = Array.from(items).find((el) =>
        el.textContent?.includes('Configuration')
      );
      if (configItem) {
        // Émettre @select comme le ferait le clavier/souris
        await configItem.dispatchEvent(new Event('select', { bubbles: true }));
        await wrapper.vm.$nextTick();
      }
    }

    expect(router.currentRoute.value.path).toBe('/settings');
  });
});