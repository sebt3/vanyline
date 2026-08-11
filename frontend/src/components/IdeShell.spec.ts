import { mount } from '@vue/test-utils';
import { describe, expect, it, vi } from 'vitest';
import IdeShell from './IdeShell.vue';

// Mock dockview-vue entirely pour éviter dockview-core (ResizeObserver) en jsdom.
vi.mock('dockview-vue', () => ({
  DockviewVue: { template: '<div class="dockview-stub" />' },
}));

describe('IdeShell', () => {
  it('reçoit la prop sandboxName', () => {
    const wrapper = mount(IdeShell, { props: { sandboxName: 'foo' } });
    expect(wrapper.props('sandboxName')).toBe('foo');
  });

  it('rend DockviewVue (mocké)', () => {
    const wrapper = mount(IdeShell, { props: { sandboxName: 'foo' } });
    expect(wrapper.find('.dockview-stub').exists()).toBe(true);
  });
});
