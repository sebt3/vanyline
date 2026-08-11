import { createMemoryHistory, createRouter } from 'vue-router';
import { mount, type VueWrapper } from '@vue/test-utils';
import { describe, expect, it } from 'vitest';
import App from '../App.vue';

describe('App', () => {
  function mountApp(): VueWrapper {
    const router = createRouter({
      history: createMemoryHistory(),
      routes: [
        { path: '/', redirect: '/settings' },
        { path: '/settings', component: { template: '<div>Settings</div>' } },
        { path: '/ide/:sandboxName', component: { template: '<div>Shell</div>' }, props: true },
      ],
    });
    return mount(App, { global: { plugins: [router] } });
  }

  it('affiche StatusBar et le workspace depuis route.params.sandboxName', async () => {
    const wrapper = mountApp();
    await wrapper.vm.$router.push('/ide/foo');
    expect(wrapper.findComponent({ name: 'StatusBar' }).props('workspace')).toBe('foo');
    expect(wrapper.text()).toContain('foo');
  });

  it('affiche MenuBar', async () => {
    const wrapper = mountApp();
    await wrapper.vm.$router.push('/ide/foo');
    expect(wrapper.findComponent({ name: 'MenuBar' }).exists()).toBe(true);
  });

  it('affiche une chaîne vide pour le workspace sur /settings', async () => {
    const wrapper = mountApp();
    await wrapper.vm.$router.push('/settings');
    expect(wrapper.findComponent({ name: 'StatusBar' }).props('workspace')).toBe('');
  });
});
