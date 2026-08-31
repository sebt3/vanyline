import { describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import Field from './Field.vue';

describe('Field', () => {
  it('affiche le label et le contenu du slot', () => {
    const wrapper = mount(Field, {
      props: { label: 'Nom' },
      slots: { default: '<input class="field-input" />' },
    });
    expect(wrapper.find('.field-label').text()).toBe('Nom');
    expect(wrapper.find('input.field-input').exists()).toBe(true);
  });

  it('sans topAlign : pas de classe field--top', () => {
    const wrapper = mount(Field, { props: { label: 'Nom' } });
    expect(wrapper.classes()).not.toContain('field--top');
  });

  it('avec topAlign : classe field--top posée', () => {
    const wrapper = mount(Field, { props: { label: 'Nom', topAlign: true } });
    expect(wrapper.classes()).toContain('field--top');
  });
});
