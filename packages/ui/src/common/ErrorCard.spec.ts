import { describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import ErrorCard from './ErrorCard.vue';

describe('ErrorCard', () => {
  it('affiche le message avec role="alert"', () => {
    const wrapper = mount(ErrorCard, { props: { message: 'HTTP 500' } });
    const card = wrapper.find('[role="alert"]');
    expect(card.exists()).toBe(true);
    expect(card.text()).toBe('HTTP 500');
  });
});
