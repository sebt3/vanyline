import { createMemoryHistory, createRouter } from 'vue-router';
import { mount, type VueWrapper } from '@vue/test-utils';
import { describe, expect, it } from 'vitest';
import StatusBar from './StatusBar.vue';
import AppBreadcrumb from './AppBreadcrumb.vue';

describe('StatusBar', () => {
  function mountStatusBar(props: { workspace?: string; extended?: boolean }): VueWrapper {
    const router = createRouter({
      history: createMemoryHistory(),
      routes: [{ path: '/', name: 'home', component: { template: '<div>Home</div>' } }],
    });
    return mount(StatusBar, { props, global: { plugins: [router] } });
  }

  it('affiche toujours le breadcrumb', () => {
    const wrapper = mountStatusBar({});
    expect(wrapper.findComponent(AppBreadcrumb).exists()).toBe(true);
  });

  it("n'affiche pas les infos étendues quand extended est absent", () => {
    const wrapper = mountStatusBar({ workspace: 'media-station' });
    expect(wrapper.text()).not.toContain('media-station');
    expect(wrapper.text()).not.toContain('⎇ main');
    expect(wrapper.text()).not.toContain('UTF-8');
  });

  it('affiche le workspace et les infos étendues quand extended est vrai', () => {
    const wrapper = mountStatusBar({ workspace: 'media-station', extended: true });
    expect(wrapper.text()).toContain('media-station');
    expect(wrapper.text()).toContain('⎇ main');
    expect(wrapper.text()).toContain('UTF-8');
  });
});
