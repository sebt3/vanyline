import { describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import EmptyState from './EmptyState.vue';

describe('EmptyState', () => {
  it('affiche le message dans une card', () => {
    const wrapper = mount(EmptyState, { props: { message: 'Aucun skill.' } });
    expect(wrapper.find('.card').text()).toBe('Aucun skill.');
  });
});
