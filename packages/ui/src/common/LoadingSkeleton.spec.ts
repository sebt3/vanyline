import { describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import LoadingSkeleton from './LoadingSkeleton.vue';

describe('LoadingSkeleton', () => {
  it('affiche le squelette de chargement (3 barres)', () => {
    const wrapper = mount(LoadingSkeleton);
    expect(wrapper.find('.skeleton-card').exists()).toBe(true);
    expect(wrapper.findAll('.skeleton').length).toBe(3);
    expect(wrapper.findAll('.skeleton.short').length).toBe(2);
  });
});