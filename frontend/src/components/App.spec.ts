import { createMemoryHistory, createRouter } from 'vue-router';
import { mount, type VueWrapper } from '@vue/test-utils';
import { describe, expect, it } from 'vitest';
import App from '../App.vue';
import MenuBar from './MenuBar.vue';
import StatusBar from './StatusBar.vue';
import AppNav from './shell/AppNav.vue';
import AppBreadcrumb from './AppBreadcrumb.vue';

describe('App', () => {
  function mountApp(): VueWrapper {
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
    return mount(App, { global: { plugins: [router] } });
  }

  it('route IDE : MenuBar, StatusBar étendue, pas de nav du shell (coquille IDE inchangée)', async () => {
    const wrapper = mountApp();
    await wrapper.vm.$router.push('/p/foo/s/bar');
    expect(wrapper.findComponent(MenuBar).exists()).toBe(true);
    expect(wrapper.findComponent(StatusBar).exists()).toBe(true);
    expect(wrapper.findComponent(StatusBar).props('extended')).toBe(true);
    expect(wrapper.findComponent(StatusBar).props('workspace')).toBe('bar');
    expect(wrapper.findComponent(AppBreadcrumb).exists()).toBe(true);
    expect(wrapper.findComponent(AppNav).exists()).toBe(false);
  });

  it('hors IDE sur / : nav « Navigation principale », pas de MenuBar ni de StatusBar, fil d’Ariane', async () => {
    const wrapper = mountApp();
    await wrapper.vm.$router.push('/');
    expect(wrapper.find('nav[aria-label="Navigation principale"]').exists()).toBe(true);
    expect(wrapper.findComponent(MenuBar).exists()).toBe(false);
    expect(wrapper.findComponent(StatusBar).exists()).toBe(false);
    expect(wrapper.findComponent(AppBreadcrumb).exists()).toBe(true);
  });

  it('hors IDE sur /p/:projectName : nav, pas de StatusBar', async () => {
    const wrapper = mountApp();
    await wrapper.vm.$router.push('/p/foo');
    expect(wrapper.findComponent(AppNav).exists()).toBe(true);
    expect(wrapper.findComponent(MenuBar).exists()).toBe(false);
    expect(wrapper.findComponent(StatusBar).exists()).toBe(false);
    expect(wrapper.findComponent(AppBreadcrumb).exists()).toBe(true);
  });

  it('hors IDE sur /settings : nav, pas de StatusBar', async () => {
    const wrapper = mountApp();
    await wrapper.vm.$router.push('/settings');
    expect(wrapper.findComponent(AppNav).exists()).toBe(true);
    expect(wrapper.findComponent(MenuBar).exists()).toBe(false);
    expect(wrapper.findComponent(StatusBar).exists()).toBe(false);
    expect(wrapper.findComponent(AppBreadcrumb).exists()).toBe(true);
  });
});
