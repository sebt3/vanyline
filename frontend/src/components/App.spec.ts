import { createMemoryHistory, createRouter } from 'vue-router';
import { mount, type VueWrapper } from '@vue/test-utils';
import { describe, expect, it } from 'vitest';
import App from '../App.vue';
import MenuBar from './MenuBar.vue';
import StatusBar from './StatusBar.vue';
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

  it('affiche MenuBar et StatusBar en mode étendu sur la route IDE', async () => {
    const wrapper = mountApp();
    await wrapper.vm.$router.push('/p/foo/s/bar');
    expect(wrapper.findComponent(MenuBar).exists()).toBe(true);
    expect(wrapper.findComponent(StatusBar).exists()).toBe(true);
    expect(wrapper.findComponent(StatusBar).props('extended')).toBe(true);
    expect(wrapper.findComponent(StatusBar).props('workspace')).toBe('bar');
    expect(wrapper.findComponent(AppBreadcrumb).exists()).toBe(true);
  });

  it('naffiche pas MenuBar, affiche StatusBar en mode non étendu sur /', async () => {
    const wrapper = mountApp();
    await wrapper.vm.$router.push('/');
    expect(wrapper.findComponent(MenuBar).exists()).toBe(false);
    expect(wrapper.findComponent(StatusBar).exists()).toBe(true);
    expect(wrapper.findComponent(StatusBar).props('extended')).toBeFalsy();
    expect(wrapper.findComponent(AppBreadcrumb).exists()).toBe(true);
  });

  it('naffiche pas MenuBar, affiche StatusBar en mode non étendu sur /p/:projectName', async () => {
    const wrapper = mountApp();
    await wrapper.vm.$router.push('/p/foo');
    expect(wrapper.findComponent(MenuBar).exists()).toBe(false);
    expect(wrapper.findComponent(StatusBar).exists()).toBe(true);
    expect(wrapper.findComponent(StatusBar).props('extended')).toBeFalsy();
    expect(wrapper.findComponent(AppBreadcrumb).exists()).toBe(true);
  });

  it('naffiche pas MenuBar, affiche StatusBar en mode non étendu sur /settings', async () => {
    const wrapper = mountApp();
    await wrapper.vm.$router.push('/settings');
    expect(wrapper.findComponent(MenuBar).exists()).toBe(false);
    expect(wrapper.findComponent(StatusBar).exists()).toBe(true);
    expect(wrapper.findComponent(StatusBar).props('extended')).toBeFalsy();
    expect(wrapper.findComponent(AppBreadcrumb).exists()).toBe(true);
  });
});
