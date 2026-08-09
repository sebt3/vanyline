import { describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import StatusBar from './StatusBar.vue';

describe('StatusBar', () => {
  it('affiche le workspace passé en prop', () => {
    const wrapper = mount(StatusBar, { props: { workspace: 'media-station' } });
    expect(wrapper.text()).toContain('media-station');
  });
});
