import { describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import CheckboxList from './CheckboxList.vue';

const options = [
  { value: 'git', label: 'git' },
  { value: 'curl', label: 'curl' },
];

describe('CheckboxList', () => {
  it('affiche une case par option, cochée si présente dans modelValue', () => {
    const wrapper = mount(CheckboxList, {
      props: { options, modelValue: ['git'] },
    });
    const inputs = wrapper.findAll('input[type="checkbox"]');
    expect(inputs.length).toBe(2);
    expect((inputs[0].element as HTMLInputElement).checked).toBe(true);
    expect((inputs[1].element as HTMLInputElement).checked).toBe(false);
    expect(wrapper.text()).toContain('git');
    expect(wrapper.text()).toContain('curl');
  });

  it('cocher une case ajoute la valeur à modelValue via update:modelValue', async () => {
    const wrapper = mount(CheckboxList, {
      props: { options, modelValue: [] },
    });
    await wrapper.findAll('input[type="checkbox"]')[1].setValue(true);
    expect(wrapper.emitted('update:modelValue')?.[0]).toEqual([['curl']]);
  });

  it('décocher une case retire la valeur de modelValue via update:modelValue', async () => {
    const wrapper = mount(CheckboxList, {
      props: { options, modelValue: ['git', 'curl'] },
    });
    await wrapper.findAll('input[type="checkbox"]')[0].setValue(false);
    expect(wrapper.emitted('update:modelValue')?.[0]).toEqual([['curl']]);
  });
});
