import { createMemoryHistory, createRouter } from 'vue-router';
import { mount, type VueWrapper } from '@vue/test-utils';
import { describe, expect, it } from 'vitest';
import AppBreadcrumb from './AppBreadcrumb.vue';

describe('AppBreadcrumb', () => {
  function mountBreadcrumb(): VueWrapper {
    const router = createRouter({
      history: createMemoryHistory(),
      routes: [
        { path: '/', name: 'home', component: { template: '<div>Home</div>' } },
        { path: '/p/:projectName', name: 'project', component: { template: '<div>Project</div>' }, props: true },
        {
          path: '/p/:projectName/s/:sandboxName',
          name: 'ide',
          component: { template: '<div>IdeShell</div>' },
          props: true,
        },
        { path: '/settings', name: 'settings', component: { template: '<div>Settings</div>' } },
      ],
    });
    return mount(AppBreadcrumb, { global: { plugins: [router] } });
  }

  it('affiche Accueil sur /', async () => {
    const wrapper = mountBreadcrumb();
    await wrapper.vm.$router.push('/');
    await wrapper.vm.$router.isReady();
    expect(wrapper.text()).toContain('Accueil');
    // Un seul segment = segment courant → texte simple, pas un lien.
    const links = wrapper.findAll('a');
    expect(links.length).toBe(0);
    expect(wrapper.find('.current').text()).toBe('Accueil');
  });

  it('affiche Accueil + projectName sur /p/:projectName', async () => {
    const wrapper = mountBreadcrumb();
    await wrapper.vm.$router.push('/p/foo');
    await wrapper.vm.$router.isReady();
    expect(wrapper.text()).toContain('Accueil');
    expect(wrapper.text()).toContain('foo');

    // Un seul lien : Accueil. "foo" est le dernier segment → texte simple, pas un lien.
    const links = wrapper.findAll('a');
    expect(links.length).toBe(1);
    expect(links[0].attributes('href')).toBe('/');
    expect(wrapper.find('.current').text()).toBe('foo');
  });

  it('affiche Accueil + Paramètres sur /settings', async () => {
    const wrapper = mountBreadcrumb();
    await wrapper.vm.$router.push('/settings');
    await wrapper.vm.$router.isReady();
    expect(wrapper.text()).toContain('Accueil');
    expect(wrapper.text()).toContain('Paramètres');

    // "Paramètres" est le dernier segment → texte simple, pas un lien.
    const links = wrapper.findAll('a');
    expect(links.length).toBe(1);
    expect(links[0].attributes('href')).toBe('/');
    expect(wrapper.find('.current').text()).toBe('Paramètres');
  });
});
