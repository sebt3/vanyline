import { describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import SourceBadge from './SourceBadge.vue';

describe('SourceBadge', () => {
  it('source absent → aucun badge rendu', () => {
    const wrapper = mount(SourceBadge);
    expect(wrapper.find('[data-testid="source-badge"]').exists()).toBe(false);
  });

  it('source undefined → aucun badge rendu', () => {
    const wrapper = mount(SourceBadge, { props: { source: undefined } });
    expect(wrapper.find('[data-testid="source-badge"]').exists()).toBe(false);
  });

  it("source 'workspace' → badge badge-source badge-source-workspace, texte workspace", () => {
    const wrapper = mount(SourceBadge, { props: { source: 'workspace' } });
    const badge = wrapper.find('[data-testid="source-badge"]');
    expect(badge.exists()).toBe(true);
    expect(badge.text()).toBe('workspace');
    expect(badge.classes()).toContain('badge-source');
    expect(badge.classes()).toContain('badge-source-workspace');
  });

  it("source 'global' → badge badge-source-global, texte global", () => {
    const wrapper = mount(SourceBadge, { props: { source: 'global' } });
    const badge = wrapper.find('[data-testid="source-badge"]');
    expect(badge.exists()).toBe(true);
    expect(badge.text()).toBe('global');
    expect(badge.classes()).toContain('badge-source');
    expect(badge.classes()).toContain('badge-source-global');
  });
});
