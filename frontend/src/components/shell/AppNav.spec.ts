// Contrat : ./shell.sdd (Scenario nav_deux_entrees, nav_entree_active).
import { createMemoryHistory, createRouter, type Router } from 'vue-router';
import { mount, type VueWrapper } from '@vue/test-utils';
import { describe, expect, it } from 'vitest';
import AppNav from './AppNav.vue';

async function mountNav(path: string): Promise<{ wrapper: VueWrapper; router: Router }> {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: '/', name: 'home', component: { template: '<div>Home</div>' } },
      { path: '/p/:projectName', name: 'project', component: { template: '<div>Project</div>' }, props: true },
      { path: '/settings', name: 'settings', component: { template: '<div>Settings</div>' } },
    ],
  });
  const wrapper = mount(AppNav, { global: { plugins: [router] } });
  await router.push(path);
  return { wrapper, router };
}

describe('AppNav — nav_deux_entrees', () => {
  it('rend deux liens « Projets » (/) et « Paramètres » (/settings) dans une nav nommée', async () => {
    const { wrapper } = await mountNav('/');
    const nav = wrapper.get('nav');
    expect(nav.attributes('aria-label')).toBe('Navigation principale');
    const links = wrapper.findAll('a');
    expect(links).toHaveLength(2);
    expect(links[0]?.text()).toContain('Projets');
    expect(links[0]?.attributes('href')).toBe('/');
    expect(links[1]?.text()).toContain('Paramètres');
    expect(links[1]?.attributes('href')).toBe('/settings');
  });
});

describe('AppNav — nav_entree_active', () => {
  it('« Projets » est active sur /p/demo, « Paramètres » seul sur /settings', async () => {
    const { wrapper, router } = await mountNav('/p/demo');
    const projets = () => wrapper.findAll('a').find((a) => a.text().includes('Projets'));
    const reglages = () => wrapper.findAll('a').find((a) => a.text().includes('Paramètres'));
    expect(projets()?.attributes('aria-current')).toBe('page');
    expect(reglages()?.attributes('aria-current')).toBeUndefined();

    await router.push('/settings');
    await wrapper.vm.$nextTick();
    expect(projets()?.attributes('aria-current')).toBeUndefined();
    expect(reglages()?.attributes('aria-current')).toBe('page');
  });

  it('les icônes sont décoratives (aria-hidden)', async () => {
    const { wrapper } = await mountNav('/');
    for (const svg of wrapper.findAll('svg')) {
      expect(svg.attributes('aria-hidden')).toBe('true');
    }
  });
});
